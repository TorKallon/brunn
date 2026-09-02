use std::{collections::BTreeMap, time::Instant};

use axum::{
    Extension, Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use chrono::{DateTime, Duration, FixedOffset, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::{
    auth::AuthContext,
    db::AppState,
    error::{ApiError, ApiResult},
    models::Capability,
};

use super::{
    rules::{
        Coordinate, Geocode, LocationReport, PoiCandidate, PresenceView, ReportDisposition,
        ReportKind, presence_view,
    },
    store::{self, ReportEvent},
};

const MAX_REPORTS_PER_BATCH: usize = 200;
const MAX_POI_PER_REPORT: usize = 5;
const RETENTION_WINDOW: Duration = Duration::days(30);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReportsRequest {
    timezone: String,
    reports: Vec<ReportRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportRequest {
    #[serde(rename = "type")]
    kind: String,
    at: DateTime<FixedOffset>,
    lat: f64,
    lon: f64,
    accuracy_m: f64,
    arrived_at: Option<DateTime<FixedOffset>>,
    departed_at: Option<DateTime<FixedOffset>>,
    geocode: Option<GeocodeRequest>,
    #[serde(default)]
    poi: Vec<PoiRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeocodeRequest {
    city: Option<String>,
    region: Option<String>,
    country: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PoiRequest {
    name: String,
    category: Option<String>,
    distance_m: f64,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReportsResponse {
    accepted: usize,
    ignored: BTreeMap<&'static str, usize>,
    presence: Option<PresenceView>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RederiveRequest {
    from: Option<DateTime<FixedOffset>>,
    to: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RederiveResponse {
    reports_replayed: usize,
    rows_written: usize,
    presence_updated: bool,
}

pub(crate) async fn reports(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    payload: Result<Json<ReportsRequest>, JsonRejection>,
) -> ApiResult<Json<ReportsResponse>> {
    auth.require(Capability::LocationWrite)?;
    let Json(payload) = payload.map_err(|_| ApiError::invalid("invalid location report batch"))?;
    let reports = validated_reports(payload)?;
    let as_of = Utc::now();
    let started = Instant::now();
    let result = store::ingest_with_retry(
        &state,
        &auth,
        &reports,
        state.config.location_pings_enabled,
        as_of,
    )
    .await;
    metrics::histogram!("brunn.location.ingest_ms")
        .record(started.elapsed().as_secs_f64() * 1_000.0);
    let result = result?;
    emit_report_metrics(&result.events);
    emit_places_warning(result.places_warning);
    let mut ignored = BTreeMap::new();
    for disposition in result.dispositions {
        let reason = match disposition {
            ReportDisposition::Accepted => continue,
            ReportDisposition::Late => "late",
            ReportDisposition::FutureClock => "future_clock",
            ReportDisposition::PingsOff => "pings_off",
            ReportDisposition::LowAccuracy => "low_accuracy",
        };
        *ignored.entry(reason).or_insert(0) += 1;
    }
    Ok(Json(ReportsResponse {
        accepted: result.accepted,
        ignored,
        presence: result
            .presence
            .as_ref()
            .map(|row| presence_view(row, as_of)),
    }))
}

pub(crate) async fn presence(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<PresenceView>> {
    auth.require(Capability::Read)?;
    let row = store::read_presence(&state, &auth).await?.ok_or_else(|| {
        ApiError::public(
            StatusCode::NOT_FOUND,
            "location_presence_not_found",
            "no location presence is available",
        )
    })?;
    Ok(Json(presence_view(&row, Utc::now())))
}

pub(crate) async fn rederive(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    payload: Result<Json<RederiveRequest>, JsonRejection>,
) -> ApiResult<Json<RederiveResponse>> {
    auth.require(Capability::Save)?;
    let Json(payload) = payload.map_err(|_| ApiError::invalid("invalid rederive window"))?;
    let as_of = Utc::now();
    let to = payload.to.map(|value| value.to_utc()).unwrap_or(as_of);
    let from = payload
        .from
        .map(|value| value.to_utc())
        .unwrap_or(as_of - RETENTION_WINDOW);
    if from > to || to > as_of || to - from > RETENTION_WINDOW || from < as_of - RETENTION_WINDOW {
        return Err(ApiError::invalid(
            "rederive window must be within the last 30 days and no longer than 30 days",
        ));
    }
    let result = store::rederive(
        &state,
        &auth,
        from,
        to,
        state.config.location_pings_enabled,
        as_of,
    )
    .await;
    metrics::counter!(
        "brunn.location.rederive",
        "outcome" => if result.is_ok() { "success" } else { "error" }
    )
    .increment(1);
    let result = result?;
    emit_places_warning(result.places_warning);
    Ok(Json(RederiveResponse {
        reports_replayed: result.reports_replayed,
        rows_written: result.rows_written,
        presence_updated: result.presence_updated,
    }))
}

fn emit_report_metrics(events: &[ReportEvent]) {
    for event in events {
        metrics::counter!(
            "brunn.location.reports",
            "type" => event.report_type,
            "outcome" => disposition_name(event.disposition)
        )
        .increment(1);
        let source = if event.report_type == "ping" {
            "ping"
        } else {
            "visit"
        };
        for transition in &event.transitions {
            metrics::counter!(
                "brunn.location.transitions",
                "kind" => transition_name(*transition),
                "source" => source
            )
            .increment(1);
        }
    }
}

fn disposition_name(disposition: ReportDisposition) -> &'static str {
    match disposition {
        ReportDisposition::Accepted => "accepted",
        ReportDisposition::Late => "late",
        ReportDisposition::FutureClock => "future_clock",
        ReportDisposition::PingsOff => "pings_off",
        ReportDisposition::LowAccuracy => "low_accuracy",
    }
}

fn transition_name(transition: super::rules::TransitionKind) -> &'static str {
    match transition {
        super::rules::TransitionKind::Open => "open",
        super::rules::TransitionKind::Close => "close",
        super::rules::TransitionKind::Completed => "completed",
        super::rules::TransitionKind::Transit => "transit",
        super::rules::TransitionKind::Duplicate => "duplicate",
    }
}

fn emit_places_warning(warning: Option<super::places::PlacesWarning>) {
    let Some(warning) = warning else {
        return;
    };
    let reason = match warning {
        super::places::PlacesWarning::Missing => "missing",
        super::places::PlacesWarning::Unparseable => "unparseable",
        super::places::PlacesWarning::InvalidRows => "invalid_rows",
    };
    tracing::warn!(reason, "location places input degraded");
}

pub(crate) async fn delete_live(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<StatusCode> {
    auth.require(Capability::LocationWrite)?;
    store::delete_live(&state, &auth).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn validated_reports(payload: ReportsRequest) -> ApiResult<Vec<LocationReport>> {
    if payload.reports.len() > MAX_REPORTS_PER_BATCH {
        return Err(ApiError::invalid(
            "a location batch may contain at most 200 reports",
        ));
    }
    let timezone = payload
        .timezone
        .parse::<Tz>()
        .map_err(|_| ApiError::invalid("timezone must be a valid IANA timezone"))?;
    payload
        .reports
        .into_iter()
        .map(|report| validated_report(report, timezone))
        .collect()
}

fn validated_report(report: ReportRequest, timezone: Tz) -> ApiResult<LocationReport> {
    if !report.lat.is_finite() || !(-90.0..=90.0).contains(&report.lat) {
        return Err(ApiError::invalid("report latitude is out of range"));
    }
    if !report.lon.is_finite() || !(-180.0..=180.0).contains(&report.lon) {
        return Err(ApiError::invalid("report longitude is out of range"));
    }
    if !report.accuracy_m.is_finite()
        || report.accuracy_m < 0.0
        || report.accuracy_m > f64::from(f32::MAX)
    {
        return Err(ApiError::invalid(
            "report accuracy must be a non-negative finite number",
        ));
    }
    if report.poi.len() > MAX_POI_PER_REPORT {
        return Err(ApiError::invalid(
            "a location report may contain at most 5 POI candidates",
        ));
    }
    for poi in &report.poi {
        if poi.name.trim().is_empty()
            || !poi.distance_m.is_finite()
            || poi.distance_m < 0.0
            || poi.distance_m > f64::from(f32::MAX)
        {
            return Err(ApiError::invalid("invalid location POI candidate"));
        }
    }

    let kind = match report.kind.as_str() {
        "ping" => {
            if report.arrived_at.is_some() || report.departed_at.is_some() || !report.poi.is_empty()
            {
                return Err(ApiError::invalid(
                    "ping reports cannot include visit dates or POI candidates",
                ));
            }
            ReportKind::Ping
        }
        "visit_arrival" => {
            if report.departed_at.is_some() {
                return Err(ApiError::invalid(
                    "visit_arrival reports cannot include departed_at",
                ));
            }
            ReportKind::VisitArrival {
                arrived_at: report.arrived_at.map(|value| value.to_utc()),
            }
        }
        "visit_departure" => {
            let arrived_at = report
                .arrived_at
                .ok_or_else(|| ApiError::invalid("visit_departure reports require arrived_at"))?;
            let departed_at = report
                .departed_at
                .ok_or_else(|| ApiError::invalid("visit_departure reports require departed_at"))?;
            if departed_at < arrived_at {
                return Err(ApiError::invalid(
                    "visit departed_at must not precede arrived_at",
                ));
            }
            ReportKind::VisitDeparture {
                arrived_at: arrived_at.to_utc(),
                departed_at: departed_at.to_utc(),
            }
        }
        _ => return Err(ApiError::invalid("unknown location report type")),
    };

    let offset_min = report.at.offset().local_minus_utc() / 60;
    let offset_min = i16::try_from(offset_min)
        .map_err(|_| ApiError::invalid("report timestamp offset is out of range"))?;
    Ok(LocationReport {
        kind,
        at: report.at.to_utc(),
        offset_min,
        timezone,
        coordinate: Coordinate {
            lat: report.lat,
            lon: report.lon,
        },
        accuracy_m: report.accuracy_m,
        geocode: report.geocode.map(|geocode| Geocode {
            city: geocode.city,
            region: geocode.region,
            country: geocode.country,
            name: geocode.name,
        }),
        poi: report
            .poi
            .into_iter()
            .map(|poi| PoiCandidate {
                name: poi.name,
                category: poi.category,
                distance_m: poi.distance_m,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(report: ReportRequest) -> ReportsRequest {
        ReportsRequest {
            timezone: "America/Los_Angeles".to_owned(),
            reports: vec![report],
        }
    }

    fn ping() -> ReportRequest {
        ReportRequest {
            kind: "ping".to_owned(),
            at: "2026-09-01T14:10:22-07:00".parse().unwrap(),
            lat: 46.9965,
            lon: -120.5478,
            accuracy_m: 65.0,
            arrived_at: None,
            departed_at: None,
            geocode: None,
            poi: Vec::new(),
        }
    }

    #[test]
    fn boundary_accepts_the_wire_ping_and_preserves_offset() {
        let parsed = validated_reports(request(ping())).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].offset_min, -420);
        assert_eq!(parsed[0].timezone, chrono_tz::America::Los_Angeles);
        assert!(matches!(parsed[0].kind, ReportKind::Ping));
    }

    #[test]
    fn boundary_rejects_invalid_semantics_for_the_whole_batch() {
        let mut invalid = ping();
        invalid.poi.push(PoiRequest {
            name: "Private Place".to_owned(),
            category: Some("other".to_owned()),
            distance_m: 10.0,
        });
        assert!(validated_reports(request(invalid)).is_err());

        let mut invalid = ping();
        invalid.accuracy_m = f64::INFINITY;
        assert!(validated_reports(request(invalid)).is_err());

        let mut invalid = ping();
        invalid.kind = "visit_departure".to_owned();
        assert!(validated_reports(request(invalid)).is_err());
    }
}

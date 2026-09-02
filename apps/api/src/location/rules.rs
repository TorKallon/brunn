use std::cmp::Ordering;

use chrono::{DateTime, Duration, FixedOffset, Utc};
use chrono_tz::Tz;
use serde::Serialize;

use super::places::KnownPlace;

pub const FUTURE_CLOCK_TOLERANCE: Duration = Duration::minutes(5);
pub const LOW_ACCURACY_M: f64 = 500.0;
pub const SAME_PLACE_MINIMUM_M: f64 = 150.0;
pub const UNKNOWN_DEPARTURE_RADIUS_M: f64 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coordinate {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geocode {
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PoiCandidate {
    pub name: String,
    pub category: Option<String>,
    pub distance_m: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReportKind {
    Ping,
    VisitArrival {
        arrived_at: Option<DateTime<Utc>>,
    },
    VisitDeparture {
        arrived_at: DateTime<Utc>,
        departed_at: DateTime<Utc>,
    },
}

impl ReportKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::VisitArrival { .. } => "visit_arrival",
            Self::VisitDeparture { .. } => "visit_departure",
        }
    }

    fn is_visit(&self) -> bool {
        !matches!(self, Self::Ping)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocationReport {
    pub kind: ReportKind,
    pub at: DateTime<Utc>,
    pub offset_min: i16,
    pub timezone: Tz,
    pub coordinate: Coordinate,
    pub accuracy_m: f64,
    pub geocode: Option<Geocode>,
    pub poi: Vec<PoiCandidate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPlace {
    pub label: Option<String>,
    pub kind: String,
    pub confidence: Confidence,
    pub known_place_index: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpenVisit {
    pub arrived_at: DateTime<Utc>,
    pub coordinate: Coordinate,
    pub label: Option<String>,
    pub kind: String,
    pub confidence: Confidence,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PresenceState {
    pub timezone: Tz,
    pub reported_at: DateTime<Utc>,
    pub last_coordinate: Coordinate,
    pub last_accuracy_m: f64,
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    pub visit: Option<OpenVisit>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRow {
    pub arrived_at: DateTime<Utc>,
    pub departed_at: Option<DateTime<Utc>>,
    pub offset_min: i16,
    pub label: Option<String>,
    pub kind: String,
    pub city: Option<String>,
    pub confidence: Confidence,
    pub coordinate: Coordinate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportDisposition {
    Accepted,
    Late,
    FutureClock,
    PingsOff,
    LowAccuracy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionKind {
    Open,
    Close,
    Completed,
    Transit,
    Duplicate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ApplyOutcome {
    pub presence: Option<PresenceState>,
    pub rows: Vec<HistoryRow>,
    pub disposition: ReportDisposition,
    pub transitions: Vec<TransitionKind>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReplayResult {
    pub presence: Option<PresenceState>,
    pub rows: Vec<HistoryRow>,
    pub dispositions: Vec<ReportDisposition>,
    pub transitions: Vec<TransitionKind>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    Stale,
    AtPlace,
    BetweenPlaces,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PresencePlaceView {
    pub label: Option<String>,
    pub kind: String,
    pub confidence: Confidence,
    pub since: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PresenceView {
    pub status: PresenceStatus,
    pub place: Option<PresencePlaceView>,
    pub at_home: bool,
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    pub timezone: String,
    pub last_seen: String,
}

pub fn resolve(report: &LocationReport, places: &[KnownPlace]) -> ResolvedPlace {
    let known = places
        .iter()
        .enumerate()
        .filter_map(|(index, place)| {
            let distance = distance_m(
                report.coordinate,
                Coordinate {
                    lat: place.lat,
                    lon: place.lon,
                },
            );
            (distance <= f64::from(place.radius_m))
                .then_some((index, distance / f64::from(place.radius_m)))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1));
    if let Some((index, _)) = known {
        let place = &places[index];
        return ResolvedPlace {
            label: Some(place.label.clone()),
            kind: place.kind.clone(),
            confidence: Confidence::High,
            known_place_index: Some(index),
        };
    }

    if report.kind.is_visit() {
        let mut poi = report.poi.iter().collect::<Vec<_>>();
        poi.sort_by(|left, right| left.distance_m.total_cmp(&right.distance_m));
        if let Some(nearest) = poi.first() {
            let unambiguous = poi
                .get(1)
                .is_none_or(|second| second.distance_m >= nearest.distance_m * 2.0);
            if nearest.distance_m <= report.accuracy_m.max(50.0) && unambiguous {
                return ResolvedPlace {
                    label: Some(nearest.name.clone()),
                    kind: nearest
                        .category
                        .clone()
                        .unwrap_or_else(|| "unknown".to_owned()),
                    confidence: Confidence::Medium,
                    known_place_index: None,
                };
            }
        }
    }

    ResolvedPlace {
        label: report
            .geocode
            .as_ref()
            .and_then(|geocode| geocode.name.clone()),
        kind: "unknown".to_owned(),
        confidence: Confidence::Low,
        known_place_index: None,
    }
}

pub fn apply(
    previous: Option<&PresenceState>,
    report: &LocationReport,
    places: &[KnownPlace],
    existing_rows: &[HistoryRow],
    pings_enabled: bool,
) -> ApplyOutcome {
    if matches!(report.kind, ReportKind::Ping) && !pings_enabled {
        return unchanged(previous, ReportDisposition::PingsOff);
    }
    if previous.is_some_and(|presence| report.at < presence.reported_at) {
        return unchanged(previous, ReportDisposition::Late);
    }

    let mut presence = refresh_presence(previous, report);
    if report.accuracy_m > LOW_ACCURACY_M {
        return ApplyOutcome {
            presence: Some(presence),
            rows: Vec::new(),
            disposition: ReportDisposition::LowAccuracy,
            transitions: Vec::new(),
        };
    }

    let resolved = resolve(report, places);
    let mut rows = Vec::new();
    let mut transitions = Vec::new();
    match &report.kind {
        ReportKind::VisitArrival { arrived_at } => {
            let same_open_visit = previous
                .and_then(|state| state.visit.as_ref())
                .is_some_and(|visit| same_place_as_resolution(visit, &resolved, report));
            if !same_open_visit {
                if let Some(state) = previous
                    && let Some(visit) = state.visit.as_ref()
                {
                    rows.push(closed_row(visit, report.at, report.offset_min, state));
                    transitions.push(TransitionKind::Close);
                }
                presence.visit = Some(open_visit(
                    arrived_at.unwrap_or(report.at),
                    report,
                    &resolved,
                ));
                transitions.push(TransitionKind::Open);
            }
        }
        ReportKind::VisitDeparture {
            arrived_at,
            departed_at,
        } => {
            let completed = completed_row(*arrived_at, *departed_at, report, &resolved, &presence);
            if existing_rows
                .iter()
                .any(|row| overlaps(&completed, row, report.accuracy_m))
            {
                transitions.push(TransitionKind::Duplicate);
            } else {
                let same_open_visit = previous
                    .and_then(|state| state.visit.as_ref())
                    .is_some_and(|visit| same_place_as_resolution(visit, &resolved, report));
                if same_open_visit {
                    let state = previous.expect("same_open_visit requires previous state");
                    let visit = state
                        .visit
                        .as_ref()
                        .expect("same_open_visit requires an open visit");
                    rows.push(closed_row(visit, *departed_at, report.offset_min, state));
                    presence.visit = None;
                    transitions.push(TransitionKind::Close);
                } else {
                    rows.push(completed);
                    transitions.push(TransitionKind::Completed);
                }
            }
        }
        ReportKind::Ping => {
            let inside_known_place = resolved.known_place_index.is_some();
            match previous.and_then(|state| state.visit.as_ref()) {
                Some(visit) if inside_known_place => {
                    if !same_place_as_resolution(visit, &resolved, report) {
                        let state = previous.expect("open visit requires previous state");
                        rows.push(closed_row(visit, report.at, report.offset_min, state));
                        transitions.push(TransitionKind::Close);
                        presence.visit = Some(open_visit(report.at, report, &resolved));
                        transitions.push(TransitionKind::Open);
                    }
                }
                Some(visit)
                    if distance_m(visit.coordinate, report.coordinate)
                        > departure_radius(visit, places) + report.accuracy_m =>
                {
                    let state = previous.expect("open visit requires previous state");
                    rows.push(closed_row(visit, report.at, report.offset_min, state));
                    presence.visit = None;
                    transitions.push(TransitionKind::Close);
                }
                Some(_) => {}
                None if inside_known_place => {
                    presence.visit = Some(open_visit(report.at, report, &resolved));
                    transitions.push(TransitionKind::Open);
                }
                None => {}
            }

            if let (Some(previous_city), Some(geocode)) = (
                previous.and_then(|state| state.city.as_deref()),
                report.geocode.as_ref(),
            ) && geocode
                .city
                .as_deref()
                .is_some_and(|city| city != previous_city)
            {
                rows.push(transit_row(report, &presence));
                transitions.push(TransitionKind::Transit);
            }
        }
    }

    ApplyOutcome {
        presence: Some(presence),
        rows,
        disposition: ReportDisposition::Accepted,
        transitions,
    }
}

pub fn replay(
    initial_presence: Option<PresenceState>,
    reports: &[LocationReport],
    places: &[KnownPlace],
    existing_rows: &[HistoryRow],
    pings_enabled: bool,
    as_of: DateTime<Utc>,
) -> ReplayResult {
    let mut ordered = reports.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.at
            .cmp(&right.at)
            .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
    });

    let mut presence = initial_presence;
    let mut rows = Vec::new();
    let mut duplicate_scope = existing_rows.to_vec();
    let mut dispositions = Vec::with_capacity(reports.len());
    let mut transitions = Vec::new();
    for report in ordered {
        if report.at > as_of + FUTURE_CLOCK_TOLERANCE {
            dispositions.push(ReportDisposition::FutureClock);
            continue;
        }
        let outcome = apply(
            presence.as_ref(),
            report,
            places,
            &duplicate_scope,
            pings_enabled,
        );
        dispositions.push(outcome.disposition);
        transitions.extend(outcome.transitions.iter().copied());
        duplicate_scope.extend(outcome.rows.iter().cloned());
        rows.extend(outcome.rows);
        presence = outcome.presence;
    }
    rows.sort_by(compare_history_rows);
    ReplayResult {
        presence,
        rows,
        dispositions,
        transitions,
    }
}

pub fn presence_view(presence: &PresenceState, now: DateTime<Utc>) -> PresenceView {
    let status = if now - presence.reported_at > Duration::hours(6) {
        PresenceStatus::Stale
    } else if presence.visit.is_some() {
        PresenceStatus::AtPlace
    } else {
        PresenceStatus::BetweenPlaces
    };
    let place = presence.visit.as_ref().map(|visit| PresencePlaceView {
        label: visit.label.clone(),
        kind: visit.kind.clone(),
        confidence: visit.confidence,
        since: render_timezone(visit.arrived_at, presence.timezone),
    });
    PresenceView {
        status,
        at_home: place.as_ref().is_some_and(|place| place.kind == "home"),
        place,
        city: presence.city.clone(),
        region: presence.region.clone(),
        country: presence.country.clone(),
        timezone: presence.timezone.to_string(),
        last_seen: render_timezone(presence.reported_at, presence.timezone),
    }
}

pub fn insert_rows(month_file_text: Option<&str>, rows: &[HistoryRow]) -> String {
    if rows.is_empty() {
        return month_file_text.unwrap_or_default().to_owned();
    }
    if let Some(text) = month_file_text
        && let Some(inserted) = splice_table_rows_preserving(text, rows, |_| true)
    {
        return inserted;
    }
    let mut combined = month_file_text.map(parse_history_rows).unwrap_or_default();
    combined.extend_from_slice(rows);
    combined.sort_by(compare_history_rows);
    render_month(
        month_from_text(month_file_text).or_else(|| combined.first().map(month_key)),
        &combined,
    )
}

pub fn replace_rows_in_window(
    month_file_text: Option<&str>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    replacement: &[HistoryRow],
) -> String {
    if let Some(text) = month_file_text
        && let Some(replaced) = splice_table_rows_preserving(text, replacement, |row| {
            row.arrived_at < from || row.arrived_at > to
        })
    {
        return replaced;
    }
    let mut rows = month_file_text
        .map(parse_history_rows)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.arrived_at < from || row.arrived_at > to)
        .collect::<Vec<_>>();
    rows.extend_from_slice(replacement);
    if rows.is_empty() && month_file_text.is_none() {
        return month_file_text.unwrap_or_default().to_owned();
    }
    rows.sort_by(compare_history_rows);
    render_month(
        month_from_text(month_file_text).or_else(|| rows.first().map(month_key)),
        &rows,
    )
}

fn splice_table_rows_preserving(
    text: &str,
    replacement: &[HistoryRow],
    retain_existing: impl Fn(&HistoryRow) -> bool,
) -> Option<String> {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let header_index = lines.iter().enumerate().find_map(|(index, line)| {
        let cells = history_cells(line)?;
        let separator = lines.get(index + 1).and_then(|line| history_cells(line))?;
        (history_header(&cells)
            && cells.len() == separator.len()
            && separator.iter().all(|cell| separator_cell(cell)))
        .then_some(index)
    })?;
    let row_start = header_index + 2;
    let mut row_end = row_start;
    let mut sortable = Vec::<(HistoryRow, String)>::new();
    let mut opaque = Vec::<String>::new();
    while let Some(line) = lines.get(row_end) {
        let Some(cells) = history_cells(line) else {
            break;
        };
        match parse_history_row(&cells) {
            Some(row) if retain_existing(&row) => {
                sortable.push((row, (*line).to_owned()));
            }
            Some(_) => {}
            None => opaque.push((*line).to_owned()),
        }
        row_end += 1;
    }
    sortable.extend(
        replacement
            .iter()
            .map(|row| (row.clone(), format!("{}\n", render_history_row(row)))),
    );
    sortable.sort_by(|(left, _), (right, _)| compare_history_rows(left, right));

    let mut result = lines[..row_start].concat();
    for (_, line) in sortable {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&line);
    }
    for line in opaque {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&line);
    }
    if row_end < lines.len() {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&lines[row_end..].concat());
    }
    Some(result)
}

pub fn parse_history_rows(text: &str) -> Vec<HistoryRow> {
    let lines = text.lines().collect::<Vec<_>>();
    let Some(header_index) = lines.iter().enumerate().find_map(|(index, line)| {
        let cells = history_cells(line)?;
        let separator = lines.get(index + 1).and_then(|line| history_cells(line))?;
        (history_header(&cells)
            && cells.len() == separator.len()
            && separator.iter().all(|cell| separator_cell(cell)))
        .then_some(index)
    }) else {
        return Vec::new();
    };
    lines
        .iter()
        .skip(header_index + 2)
        .map_while(|line| history_cells(line))
        .filter_map(|cells| parse_history_row(&cells))
        .collect()
}

pub fn month_key(row: &HistoryRow) -> String {
    fixed_time(row.arrived_at, row.offset_min)
        .format("%Y-%m")
        .to_string()
}

pub fn distance_m(left: Coordinate, right: Coordinate) -> f64 {
    const EARTH_RADIUS_M: f64 = 6_371_000.0;
    let left_lat = left.lat.to_radians();
    let right_lat = right.lat.to_radians();
    let delta_lat = (right.lat - left.lat).to_radians();
    let delta_lon = (right.lon - left.lon).to_radians();
    let a = (delta_lat / 2.0).sin().powi(2)
        + left_lat.cos() * right_lat.cos() * (delta_lon / 2.0).sin().powi(2);
    EARTH_RADIUS_M * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
}

fn unchanged(previous: Option<&PresenceState>, disposition: ReportDisposition) -> ApplyOutcome {
    ApplyOutcome {
        presence: previous.cloned(),
        rows: Vec::new(),
        disposition,
        transitions: Vec::new(),
    }
}

fn refresh_presence(previous: Option<&PresenceState>, report: &LocationReport) -> PresenceState {
    let mut presence = previous.cloned().unwrap_or(PresenceState {
        timezone: report.timezone,
        reported_at: report.at,
        last_coordinate: report.coordinate,
        last_accuracy_m: report.accuracy_m,
        city: None,
        region: None,
        country: None,
        visit: None,
    });
    presence.timezone = report.timezone;
    presence.reported_at = report.at;
    presence.last_coordinate = report.coordinate;
    presence.last_accuracy_m = report.accuracy_m;
    if let Some(geocode) = report.geocode.as_ref() {
        if let Some(city) = geocode.city.as_ref() {
            presence.city = Some(city.clone());
        }
        if let Some(region) = geocode.region.as_ref() {
            presence.region = Some(region.clone());
        }
        if let Some(country) = geocode.country.as_ref() {
            presence.country = Some(country.clone());
        }
    }
    presence
}

fn open_visit(
    arrived_at: DateTime<Utc>,
    report: &LocationReport,
    resolved: &ResolvedPlace,
) -> OpenVisit {
    OpenVisit {
        arrived_at,
        coordinate: report.coordinate,
        label: resolved.label.clone(),
        kind: resolved.kind.clone(),
        confidence: resolved.confidence,
    }
}

fn closed_row(
    visit: &OpenVisit,
    departed_at: DateTime<Utc>,
    offset_min: i16,
    presence: &PresenceState,
) -> HistoryRow {
    HistoryRow {
        arrived_at: visit.arrived_at,
        departed_at: Some(departed_at),
        offset_min,
        label: visit.label.clone(),
        kind: visit.kind.clone(),
        city: city_display(presence),
        confidence: visit.confidence,
        coordinate: visit.coordinate,
    }
}

fn completed_row(
    arrived_at: DateTime<Utc>,
    departed_at: DateTime<Utc>,
    report: &LocationReport,
    resolved: &ResolvedPlace,
    presence: &PresenceState,
) -> HistoryRow {
    HistoryRow {
        arrived_at,
        departed_at: Some(departed_at),
        offset_min: report.offset_min,
        label: resolved.label.clone(),
        kind: resolved.kind.clone(),
        city: city_display(presence),
        confidence: resolved.confidence,
        coordinate: report.coordinate,
    }
}

fn transit_row(report: &LocationReport, presence: &PresenceState) -> HistoryRow {
    HistoryRow {
        arrived_at: report.at,
        departed_at: None,
        offset_min: report.offset_min,
        label: None,
        kind: "transit".to_owned(),
        city: city_display(presence),
        confidence: Confidence::Low,
        coordinate: report.coordinate,
    }
}

fn city_display(presence: &PresenceState) -> Option<String> {
    let parts = [
        presence.city.as_deref(),
        presence.region.as_deref(),
        presence.country.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn same_place_as_resolution(
    visit: &OpenVisit,
    resolved: &ResolvedPlace,
    report: &LocationReport,
) -> bool {
    same_location(
        visit.label.as_deref(),
        visit.coordinate,
        resolved.label.as_deref(),
        report.coordinate,
        report.accuracy_m,
    )
}

fn same_location(
    left_label: Option<&str>,
    left_coordinate: Coordinate,
    right_label: Option<&str>,
    right_coordinate: Coordinate,
    accuracy_m: f64,
) -> bool {
    match (left_label, right_label) {
        (Some(left), Some(right)) => left == right,
        _ => {
            distance_m(left_coordinate, right_coordinate)
                < SAME_PLACE_MINIMUM_M.max(2.0 * accuracy_m)
        }
    }
}

fn departure_radius(visit: &OpenVisit, places: &[KnownPlace]) -> f64 {
    visit
        .label
        .as_deref()
        .and_then(|label| places.iter().find(|place| place.label == label))
        .map_or(UNKNOWN_DEPARTURE_RADIUS_M, |place| {
            f64::from(place.radius_m)
        })
}

fn overlaps(left: &HistoryRow, right: &HistoryRow, accuracy_m: f64) -> bool {
    if !same_location(
        left.label.as_deref(),
        left.coordinate,
        right.label.as_deref(),
        right.coordinate,
        accuracy_m,
    ) {
        return false;
    }
    let left_end = left.departed_at.unwrap_or(left.arrived_at);
    let right_end = right.departed_at.unwrap_or(right.arrived_at);
    left.arrived_at <= right_end && right.arrived_at <= left_end
}

fn compare_history_rows(left: &HistoryRow, right: &HistoryRow) -> Ordering {
    left.arrived_at
        .cmp(&right.arrived_at)
        .then_with(|| left.departed_at.cmp(&right.departed_at))
        .then_with(|| left.kind.cmp(&right.kind))
        .then_with(|| left.label.cmp(&right.label))
        .then_with(|| left.coordinate.lat.total_cmp(&right.coordinate.lat))
        .then_with(|| left.coordinate.lon.total_cmp(&right.coordinate.lon))
}

fn render_month(month: Option<String>, rows: &[HistoryRow]) -> String {
    let month = month.unwrap_or_else(|| "unknown".to_owned());
    let mut text = format!(
        "---\nkind: location-visits\nmonth: {month}\n---\n\
         | Arrived | Departed | Dwell | Place | Kind | City | Conf | Coord |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- |\n"
    );
    for row in rows {
        text.push_str(&render_history_row(row));
        text.push('\n');
    }
    text
}

fn render_history_row(row: &HistoryRow) -> String {
    let arrived = render_fixed(row.arrived_at, row.offset_min);
    let departed = row
        .departed_at
        .map(|value| render_fixed(value, row.offset_min))
        .unwrap_or_else(|| "—".to_owned());
    let dwell = row
        .departed_at
        .map(|value| render_dwell(value - row.arrived_at))
        .unwrap_or_else(|| "—".to_owned());
    let place = if row.kind == "transit" {
        "passed through"
    } else {
        row.label.as_deref().unwrap_or("—")
    };
    format!(
        "| {arrived} | {departed} | {dwell} | {place} | {} | {} | {} | {:.4},{:.4} |",
        row.kind,
        row.city.as_deref().unwrap_or("—"),
        row.confidence.as_str(),
        row.coordinate.lat,
        row.coordinate.lon,
    )
}

fn render_dwell(duration: Duration) -> String {
    let minutes = duration.num_minutes().max(0);
    if minutes >= 24 * 60 {
        format!("{}d{}h", minutes / (24 * 60), minutes % (24 * 60) / 60)
    } else if minutes >= 60 {
        format!("{}h{:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

fn render_fixed(value: DateTime<Utc>, offset_min: i16) -> String {
    fixed_time(value, offset_min)
        .format("%Y-%m-%dT%H:%M%:z")
        .to_string()
}

fn fixed_time(value: DateTime<Utc>, offset_min: i16) -> DateTime<FixedOffset> {
    let offset = FixedOffset::east_opt(i32::from(offset_min) * 60)
        .unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC offset exists"));
    value.with_timezone(&offset)
}

fn render_timezone(value: DateTime<Utc>, timezone: Tz) -> String {
    value
        .with_timezone(&timezone)
        .format("%Y-%m-%dT%H:%M%:z")
        .to_string()
}

fn month_from_text(text: Option<&str>) -> Option<String> {
    text?.lines().find_map(|line| {
        line.trim()
            .strip_prefix("month:")
            .map(str::trim)
            .filter(|month| !month.is_empty())
            .map(str::to_owned)
    })
}

fn history_cells(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('|').unwrap_or(trimmed);
    let cells = trimmed.split('|').map(str::trim).collect::<Vec<_>>();
    (cells.len() == 8).then_some(cells)
}

fn history_header(cells: &[&str]) -> bool {
    let expected = [
        "Arrived", "Departed", "Dwell", "Place", "Kind", "City", "Conf", "Coord",
    ];
    cells
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
}

fn separator_cell(cell: &str) -> bool {
    let cell = cell.trim().trim_start_matches(':').trim_end_matches(':');
    cell.len() >= 3 && cell.bytes().all(|byte| byte == b'-')
}

fn parse_history_row(cells: &[&str]) -> Option<HistoryRow> {
    let arrived = parse_history_timestamp(cells.first()?)?;
    let offset_min = i16::try_from(arrived.offset().local_minus_utc() / 60).ok()?;
    let arrived_at = arrived.with_timezone(&Utc);
    let departed_at = match *cells.get(1)? {
        "—" => None,
        value => Some(parse_history_timestamp(value)?.with_timezone(&Utc)),
    };
    let kind = cells.get(4)?.to_string();
    let label = match *cells.get(3)? {
        "—" | "passed through" => None,
        value => Some(value.to_owned()),
    };
    let city = match *cells.get(5)? {
        "—" => None,
        value => Some(value.to_owned()),
    };
    let confidence = match *cells.get(6)? {
        "high" => Confidence::High,
        "medium" => Confidence::Medium,
        "low" => Confidence::Low,
        _ => return None,
    };
    let (lat, lon) = cells.get(7)?.split_once(',')?;
    Some(HistoryRow {
        arrived_at,
        departed_at,
        offset_min,
        label,
        kind,
        city,
        confidence,
        coordinate: Coordinate {
            lat: lat.trim().parse().ok()?,
            lon: lon.trim().parse().ok()?,
        },
    })
}

fn parse_history_timestamp(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_str(value, "%Y-%m-%dT%H:%M%:z")
        .or_else(|_| DateTime::parse_from_rfc3339(value))
        .ok()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;

    const BELLEVUE: Coordinate = Coordinate {
        lat: 47.6156,
        lon: -122.2035,
    };
    const HOME: Coordinate = Coordinate {
        lat: 47.6205,
        lon: -122.2070,
    };
    const NEAR_HOME: Coordinate = Coordinate {
        lat: 47.6213,
        lon: -122.2070,
    };

    fn at(hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 1, hour, minute, 0)
            .single()
            .unwrap()
    }

    fn geocode(city: &str, name: Option<&str>) -> Geocode {
        Geocode {
            city: Some(city.to_owned()),
            region: Some("WA".to_owned()),
            country: Some("US".to_owned()),
            name: name.map(str::to_owned),
        }
    }

    fn report(kind: ReportKind, at: DateTime<Utc>, coordinate: Coordinate) -> LocationReport {
        LocationReport {
            kind,
            at,
            offset_min: -7 * 60,
            timezone: chrono_tz::America::Los_Angeles,
            coordinate,
            accuracy_m: 25.0,
            geocode: None,
            poi: Vec::new(),
        }
    }

    fn known_places(home_radius: u16) -> Vec<KnownPlace> {
        vec![KnownPlace {
            label: "Home".to_owned(),
            kind: "home".to_owned(),
            lat: HOME.lat,
            lon: HOME.lon,
            radius_m: home_radius,
        }]
    }

    fn replay_day_reports() -> (Vec<LocationReport>, LocationReport) {
        let home_ping = LocationReport {
            geocode: Some(geocode("Bellevue", None)),
            ..report(ReportKind::Ping, at(9, 10), HOME)
        };
        let drive_ping = LocationReport {
            accuracy_m: 65.0,
            geocode: Some(geocode("Ellensburg", None)),
            ..report(
                ReportKind::Ping,
                at(12, 42),
                Coordinate {
                    lat: 46.9965,
                    lon: -120.5478,
                },
            )
        };
        let restaurant = LocationReport {
            geocode: Some(geocode("Bellevue", Some("Bellevue Square"))),
            poi: vec![
                PoiCandidate {
                    name: "Bellevue Square".to_owned(),
                    category: Some("store".to_owned()),
                    distance_m: 95.0,
                },
                PoiCandidate {
                    name: "Din Tai Fung".to_owned(),
                    category: Some("restaurant".to_owned()),
                    distance_m: 18.0,
                },
            ],
            ..report(
                ReportKind::VisitDeparture {
                    arrived_at: at(12, 55),
                    departed_at: at(13, 40),
                },
                at(13, 45),
                BELLEVUE,
            )
        };
        let home_again = LocationReport {
            geocode: Some(geocode("Bellevue", None)),
            ..report(ReportKind::Ping, at(14, 10), NEAR_HOME)
        };
        let apple_home = LocationReport {
            geocode: Some(geocode("Bellevue", Some("Home"))),
            ..report(
                ReportKind::VisitDeparture {
                    arrived_at: at(9, 10),
                    departed_at: at(12, 42),
                },
                at(14, 15),
                HOME,
            )
        };
        let late = LocationReport {
            geocode: Some(geocode("Bellevue", Some("Neighborhood"))),
            ..report(
                ReportKind::VisitDeparture {
                    arrived_at: at(13, 50),
                    departed_at: at(14, 0),
                },
                at(14, 5),
                NEAR_HOME,
            )
        };
        (
            vec![home_again, restaurant, home_ping, apple_home, drive_ping],
            late,
        )
    }

    #[test]
    fn resolve_uses_known_normalized_distance_then_poi_then_geocode() {
        let mut visit = report(
            ReportKind::VisitArrival { arrived_at: None },
            at(12, 0),
            BELLEVUE,
        );
        let places = vec![
            KnownPlace {
                label: "Wide".to_owned(),
                kind: "other".to_owned(),
                lat: 47.6156,
                lon: -122.2030,
                radius_m: 500,
            },
            KnownPlace {
                label: "Tight".to_owned(),
                kind: "restaurant".to_owned(),
                lat: 47.6156,
                lon: -122.2035,
                radius_m: 50,
            },
        ];
        assert_eq!(resolve(&visit, &places).label.as_deref(), Some("Tight"));

        visit.poi = vec![
            PoiCandidate {
                name: "Bellevue Square".to_owned(),
                category: Some("store".to_owned()),
                distance_m: 95.0,
            },
            PoiCandidate {
                name: "Din Tai Fung".to_owned(),
                category: Some("restaurant".to_owned()),
                distance_m: 18.0,
            },
        ];
        assert_eq!(resolve(&visit, &[]).confidence, Confidence::Medium);
        assert_eq!(resolve(&visit, &[]).label.as_deref(), Some("Din Tai Fung"));

        visit.poi[0].distance_m = 30.0;
        visit.geocode = Some(geocode("Bellevue", Some("Bellevue Square")));
        let resolved = resolve(&visit, &[]);
        assert_eq!(resolved.confidence, Confidence::Low);
        assert_eq!(resolved.label.as_deref(), Some("Bellevue Square"));

        visit.kind = ReportKind::Ping;
        visit.poi[0].distance_m = 95.0;
        assert_eq!(resolve(&visit, &[]).confidence, Confidence::Low);
    }

    #[test]
    fn replay_day_is_deterministic_and_idempotent_with_pings_enabled() {
        let (reports, late_report) = replay_day_reports();
        let first = replay(None, &reports, &known_places(150), &[], true, at(15, 0));

        assert_eq!(first.rows.len(), 3);
        assert_eq!(first.rows[0].label.as_deref(), Some("Home"));
        assert_eq!(first.rows[1].kind, "transit");
        assert_eq!(first.rows[2].label.as_deref(), Some("Din Tai Fung"));
        assert_eq!(
            first.transitions,
            vec![
                TransitionKind::Open,
                TransitionKind::Close,
                TransitionKind::Transit,
                TransitionKind::Completed,
                TransitionKind::Open,
                TransitionKind::Duplicate,
            ]
        );
        let presence = first.presence.as_ref().unwrap();
        assert_eq!(
            presence.visit.as_ref().unwrap().label.as_deref(),
            Some("Home")
        );
        assert_eq!(presence.visit.as_ref().unwrap().arrived_at, at(14, 10));

        let late = apply(
            first.presence.as_ref(),
            &late_report,
            &known_places(150),
            &first.rows,
            true,
        );
        assert!(late_report.at < presence.reported_at);
        assert_eq!(late.disposition, ReportDisposition::Late);
        assert_eq!(late.presence, first.presence);
        assert_eq!(
            first
                .dispositions
                .iter()
                .chain(std::iter::once(&late.disposition))
                .filter(|disposition| **disposition == ReportDisposition::Late)
                .count(),
            1
        );

        let mut all_reports = reports.clone();
        all_reports.push(late_report);
        let replayed = replay(
            first.presence.clone(),
            &all_reports,
            &known_places(150),
            &first.rows,
            true,
            at(15, 0),
        );
        assert!(replayed.rows.is_empty());
        assert_eq!(replayed.presence, first.presence);
    }

    #[test]
    fn replay_day_with_pings_disabled_uses_only_visit_reports() {
        let (mut reports, late_report) = replay_day_reports();
        reports.push(late_report);
        let result = replay(None, &reports, &known_places(150), &[], false, at(15, 0));

        let pings_off = result
            .dispositions
            .iter()
            .filter(|outcome| **outcome == ReportDisposition::PingsOff)
            .count();
        let raw_eligible_visits = reports
            .iter()
            .filter(|report| report.kind.is_visit())
            .count();
        assert_eq!(pings_off, 3);
        assert_eq!(raw_eligible_visits, 3);
        assert_eq!(result.rows.len(), raw_eligible_visits);
        assert!(
            result
                .rows
                .iter()
                .any(|row| row.label.as_deref() == Some("Home"))
        );
        assert!(!result.transitions.contains(&TransitionKind::Transit));

        let replayed = replay(
            result.presence.clone(),
            &reports,
            &known_places(150),
            &result.rows,
            false,
            at(15, 0),
        );
        assert!(replayed.rows.is_empty());
        assert_eq!(replayed.presence, result.presence);
    }

    #[test]
    fn late_future_and_low_accuracy_reports_have_exact_boundary_behavior() {
        let current = replay(
            None,
            &[LocationReport {
                geocode: Some(geocode("Bellevue", None)),
                ..report(ReportKind::Ping, at(14, 0), HOME)
            }],
            &known_places(150),
            &[],
            true,
            at(14, 0),
        )
        .presence
        .unwrap();

        let late = report(ReportKind::Ping, at(13, 59), BELLEVUE);
        let late_outcome = apply(Some(&current), &late, &known_places(150), &[], true);
        assert_eq!(late_outcome.disposition, ReportDisposition::Late);
        assert_eq!(late_outcome.presence.as_ref(), Some(&current));
        let rederived_late = replay(None, &[late], &known_places(150), &[], true, at(14, 0));
        assert_eq!(
            rederived_late.dispositions,
            vec![ReportDisposition::Accepted]
        );
        assert_eq!(
            rederived_late.presence.as_ref().unwrap().reported_at,
            at(13, 59)
        );

        let low_accuracy = LocationReport {
            accuracy_m: 500.1,
            geocode: Some(Geocode {
                city: Some("Seattle".to_owned()),
                region: None,
                country: None,
                name: None,
            }),
            ..report(ReportKind::Ping, at(14, 1), BELLEVUE)
        };
        let low_outcome = apply(Some(&current), &low_accuracy, &known_places(150), &[], true);
        assert_eq!(low_outcome.disposition, ReportDisposition::LowAccuracy);
        assert!(low_outcome.rows.is_empty());
        assert_eq!(
            low_outcome
                .presence
                .as_ref()
                .unwrap()
                .visit
                .as_ref()
                .unwrap()
                .arrived_at,
            at(14, 0)
        );
        assert_eq!(
            low_outcome.presence.as_ref().unwrap().city.as_deref(),
            Some("Seattle")
        );
        assert_eq!(
            low_outcome.presence.as_ref().unwrap().region.as_deref(),
            Some("WA")
        );
        assert_eq!(
            low_outcome.presence.as_ref().unwrap().country.as_deref(),
            Some("US")
        );

        let future = report(ReportKind::Ping, at(14, 6), BELLEVUE);
        let future_result = replay(
            Some(current.clone()),
            &[future],
            &known_places(150),
            &[],
            true,
            at(14, 0),
        );
        assert_eq!(
            future_result.dispositions,
            vec![ReportDisposition::FutureClock]
        );
        assert_eq!(future_result.presence.as_ref(), Some(&current));
    }

    #[test]
    fn replay_day_rederive_with_changed_home_radius_places_late_report_and_is_idempotent() {
        let outside = HistoryRow {
            arrived_at: at(8, 0),
            departed_at: Some(at(8, 30)),
            offset_min: -7 * 60,
            label: Some("Outside".to_owned()),
            kind: "other".to_owned(),
            city: Some("Seattle, WA, US".to_owned()),
            confidence: Confidence::Low,
            coordinate: BELLEVUE,
        };
        let (reports, late_report) = replay_day_reports();
        let live = replay(None, &reports, &known_places(150), &[], true, at(15, 0));
        let live_late = apply(
            live.presence.as_ref(),
            &late_report,
            &known_places(150),
            &live.rows,
            true,
        );
        assert_eq!(live_late.disposition, ReportDisposition::Late);
        assert_eq!(live_late.presence, live.presence);
        assert!(live_late.rows.is_empty());
        let canonical_outside_line = render_history_row(&outside);
        let owner_edited_outside_line = canonical_outside_line
            .replace("| 30m |", "|   30m   |")
            .replace("| 47.6156,-122.2035 |", "| 47.61560,-122.20350 |");
        assert_ne!(owner_edited_outside_line, canonical_outside_line);
        let existing = insert_rows(
            None,
            &std::iter::once(outside.clone())
                .chain(live.rows.iter().cloned())
                .collect::<Vec<_>>(),
        )
        .replace(&canonical_outside_line, &owner_edited_outside_line);

        let mut raw_reports = reports;
        raw_reports.push(late_report);
        let before_radius_change =
            replay(None, &raw_reports, &known_places(150), &[], true, at(15, 0));
        let derived = replay(None, &raw_reports, &known_places(50), &[], true, at(15, 0));
        let before_late_row = before_radius_change
            .rows
            .iter()
            .find(|row| row.arrived_at == at(13, 50))
            .unwrap();
        let derived_late_row = derived
            .rows
            .iter()
            .find(|row| row.arrived_at == at(13, 50))
            .unwrap();
        assert_eq!(before_late_row.label.as_deref(), Some("Home"));
        assert_eq!(before_late_row.confidence, Confidence::High);
        assert_eq!(derived_late_row.label.as_deref(), Some("Neighborhood"));
        assert_eq!(derived_late_row.confidence, Confidence::Low);
        assert!(before_radius_change.presence.unwrap().visit.is_some());
        let derived_presence = derived.presence.as_ref().unwrap();
        assert_eq!(derived_presence.reported_at, at(14, 15));
        assert!(derived_presence.visit.is_none());
        assert_ne!(derived.presence, live.presence);

        let replaced = replace_rows_in_window(Some(&existing), at(9, 0), at(15, 0), &derived.rows);
        let rows = parse_history_rows(&replaced);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0], outside);
        assert!(existing.contains(&owner_edited_outside_line));
        assert!(replaced.contains(&owner_edited_outside_line));
        assert_eq!(replaced.matches(&owner_edited_outside_line).count(), 1);
        assert_eq!(
            rows.iter()
                .find(|row| row.arrived_at == at(13, 50))
                .unwrap()
                .label
                .as_deref(),
            Some("Neighborhood")
        );
        assert_eq!(
            replace_rows_in_window(Some(&replaced), at(9, 0), at(15, 0), &derived.rows,),
            replaced
        );
    }

    #[test]
    fn month_rendering_is_chronological_and_uses_the_contract_format() {
        let later = HistoryRow {
            arrived_at: at(12, 55),
            departed_at: Some(at(13, 40)),
            offset_min: -7 * 60,
            label: Some("Din Tai Fung".to_owned()),
            kind: "restaurant".to_owned(),
            city: Some("Bellevue, WA, US".to_owned()),
            confidence: Confidence::Medium,
            coordinate: BELLEVUE,
        };
        let earlier = HistoryRow {
            arrived_at: at(9, 10),
            departed_at: Some(at(12, 42)),
            offset_min: -7 * 60,
            label: Some("Home".to_owned()),
            kind: "home".to_owned(),
            city: Some("Bellevue, WA, US".to_owned()),
            confidence: Confidence::High,
            coordinate: HOME,
        };
        let text = insert_rows(None, &[later, earlier]);
        assert!(text.contains("month: 2026-09"));
        assert!(text.contains("3h32m | Home | home | Bellevue, WA, US | high"));
        assert!(text.contains("45m | Din Tai Fung | restaurant"));
        assert!(text.find("Home").unwrap() < text.find("Din Tai Fung").unwrap());
        assert_eq!(parse_history_rows(&text).len(), 2);

        let header_only = replace_rows_in_window(Some(&text), at(9, 0), at(14, 0), &[]);
        assert!(header_only.contains("kind: location-visits\nmonth: 2026-09"));
        assert!(header_only.ends_with("| --- | --- | --- | --- | --- | --- | --- | --- |\n"));
        assert!(parse_history_rows(&header_only).is_empty());
    }

    #[test]
    fn insert_rows_preserves_owner_content_and_existing_row_bytes() {
        let existing_row = HistoryRow {
            arrived_at: at(10, 0),
            departed_at: Some(at(10, 30)),
            offset_min: -7 * 60,
            label: Some("Owner Place".to_owned()),
            kind: "custom".to_owned(),
            city: Some("Owner City".to_owned()),
            confidence: Confidence::Low,
            coordinate: BELLEVUE,
        };
        let canonical_existing_line = render_history_row(&existing_row);
        let noncanonical_existing_line = canonical_existing_line
            .replace("| 30m |", "|   owner dwell note   |")
            .replace("| 47.6156,-122.2035 |", "| 47.615600,-122.203500 |");
        assert_ne!(noncanonical_existing_line, canonical_existing_line);
        let prefix = concat!(
            "---\n",
            "kind: location-visits\n",
            "month: 2026-09\n",
            "owner_extension: keep-this-byte-for-byte\n",
            "---\n\n",
            "Owner prose before the generated history.\n",
            "<!-- owner comment before table -->\n",
            "| Arrived | Departed | Dwell | Place | Kind | City | Conf | Coord |\n",
            "| --- | --- | --- | --- | --- | --- | --- | --- |\n",
        );
        let suffix = concat!(
            "<!-- owner comment after table -->\n",
            "Owner prose after the generated history.\n",
        );
        let existing = format!("{prefix}{noncanonical_existing_line}\n{suffix}");
        let added = HistoryRow {
            arrived_at: at(9, 0),
            departed_at: Some(at(9, 20)),
            offset_min: -7 * 60,
            label: Some("Generated Place".to_owned()),
            kind: "other".to_owned(),
            city: Some("Generated City".to_owned()),
            confidence: Confidence::Medium,
            coordinate: HOME,
        };
        let added_line = format!("{}\n", render_history_row(&added));

        let inserted = insert_rows(Some(&existing), &[added]);

        assert_eq!(inserted.matches(&noncanonical_existing_line).count(), 1);
        assert!(
            inserted.find(&added_line).unwrap()
                < inserted.find(&noncanonical_existing_line).unwrap()
        );
        assert_eq!(inserted.replacen(&added_line, "", 1), existing);
        assert_eq!(parse_history_rows(&inserted).len(), 2);
    }

    #[test]
    fn presence_view_computes_status_without_discarding_stale_fields() {
        let state = PresenceState {
            timezone: chrono_tz::America::Los_Angeles,
            reported_at: at(18, 42),
            last_coordinate: HOME,
            last_accuracy_m: 20.0,
            city: Some("Bellevue".to_owned()),
            region: Some("WA".to_owned()),
            country: Some("US".to_owned()),
            visit: Some(OpenVisit {
                arrived_at: at(16, 10),
                coordinate: HOME,
                label: Some("Home".to_owned()),
                kind: "home".to_owned(),
                confidence: Confidence::High,
            }),
        };
        let live = presence_view(&state, at(20, 0));
        assert_eq!(live.status, PresenceStatus::AtPlace);
        assert!(live.at_home);
        assert_eq!(live.place.as_ref().unwrap().since, "2026-09-01T09:10-07:00");

        let stale = presence_view(&state, at(18, 43) + Duration::hours(6));
        assert_eq!(stale.status, PresenceStatus::Stale);
        assert_eq!(stale.place, live.place);
        assert_eq!(stale.city, live.city);
    }
}

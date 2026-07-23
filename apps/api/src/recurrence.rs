use chrono::{DateTime, LocalResult, TimeZone, Utc};
use rrule::{RRuleSet, Tz};

use crate::{
    error::{ApiError, ApiResult},
    models::TemporalSpecV1,
};

#[derive(Clone, Debug)]
pub struct Expansion {
    pub dates: Vec<DateTime<Tz>>,
    pub limited: bool,
}

pub fn start_datetime(spec: &TemporalSpecV1) -> ApiResult<DateTime<Tz>> {
    match spec {
        TemporalSpecV1::Date { date } | TemporalSpecV1::DateInterval { start: date, .. } => Tz::UTC
            .from_local_datetime(
                &date
                    .and_hms_opt(0, 0, 0)
                    .ok_or_else(|| ApiError::invalid("invalid all-day recurrence start"))?,
            )
            .single()
            .ok_or_else(|| ApiError::invalid("invalid all-day recurrence start")),
        TemporalSpecV1::LocalDatetime { local, time_zone }
        | TemporalSpecV1::Due { local, time_zone } => local_in_zone(*local, time_zone),
        TemporalSpecV1::FloatingDatetime { local } => Tz::UTC
            .from_local_datetime(local)
            .single()
            .ok_or_else(|| ApiError::invalid("invalid floating recurrence start")),
        TemporalSpecV1::Instant { at } | TemporalSpecV1::InstantInterval { start: at, .. } => {
            Ok(at.with_timezone(&Tz::UTC))
        }
        TemporalSpecV1::LocalInterval {
            start_local,
            time_zone,
            ..
        } => local_in_zone(*start_local, time_zone),
    }
}

fn local_in_zone(local: chrono::NaiveDateTime, time_zone: &str) -> ApiResult<DateTime<Tz>> {
    let zone = time_zone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| ApiError::invalid(format!("unknown IANA time zone: {time_zone}")))?;
    match zone.from_local_datetime(&local) {
        LocalResult::Single(value) => Ok(value.with_timezone(&Tz::from(zone))),
        LocalResult::Ambiguous(_, _) => Err(ApiError::invalid(format!(
            "recurrence start {local} is ambiguous in {time_zone}"
        ))),
        LocalResult::None => Err(ApiError::invalid(format!(
            "recurrence start {local} does not exist in {time_zone}"
        ))),
    }
}

pub fn expand(
    start: DateTime<Tz>,
    rules: &[String],
    exclusions: Vec<DateTime<Tz>>,
    window_start: Option<DateTime<Utc>>,
    window_end: Option<DateTime<Utc>>,
    limit: usize,
) -> ApiResult<Expansion> {
    if rules.is_empty() {
        return Err(ApiError::invalid(
            "recurrence expansion requires at least one RFC 5545 rule",
        ));
    }
    if limit == 0 || limit > u16::MAX as usize {
        return Err(ApiError::invalid(format!(
            "recurrence expansion limit must be between 1 and {}",
            u16::MAX
        )));
    }
    if let (Some(start), Some(end)) = (window_start, window_end)
        && end <= start
    {
        return Err(ApiError::invalid(
            "recurrence window_end must be after window_start",
        ));
    }

    let content = rules
        .iter()
        .map(|rule| {
            let rule = rule.trim();
            if rule.starts_with("RRULE:") {
                rule.to_owned()
            } else {
                format!("RRULE:{rule}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut set = RRuleSet::new(start)
        .set_from_string(&content)
        .map_err(|error| ApiError::invalid(format!("invalid RFC 5545 recurrence: {error}")))?
        .set_exdates(exclusions);
    if let Some(start) = window_start {
        set = set.after(start.with_timezone(&Tz::UTC));
    }
    if let Some(end) = window_end {
        set = set.before(end.with_timezone(&Tz::UTC));
    }
    let result = set.all(limit as u16);
    Ok(Expansion {
        dates: result.dates,
        limited: result.limited,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, TimeZone, Timelike};

    use super::*;

    #[test]
    fn expands_across_dst_in_the_civil_time_zone() {
        let zone = Tz::America__Los_Angeles;
        let start = zone.with_ymd_and_hms(2026, 3, 7, 9, 0, 0).unwrap();
        let result = expand(
            start,
            &["FREQ=DAILY;COUNT=3".to_owned()],
            vec![],
            None,
            None,
            10,
        )
        .unwrap();

        assert_eq!(result.dates.len(), 3);
        assert!(result.dates.iter().all(|date| date.hour() == 9));
        assert_eq!(result.dates[0].offset().to_string(), "PST");
        assert_eq!(result.dates[1].offset().to_string(), "PDT");
    }

    #[test]
    fn applies_exclusions_and_bounded_windows() {
        let start = Tz::UTC.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let excluded = Tz::UTC.with_ymd_and_hms(2026, 1, 3, 12, 0, 0).unwrap();
        let result = expand(
            start,
            &["FREQ=DAILY;COUNT=10".to_owned()],
            vec![excluded],
            Some(Utc.with_ymd_and_hms(2026, 1, 1, 23, 59, 59).unwrap()),
            Some(Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap()),
            10,
        )
        .unwrap();

        let days = result
            .dates
            .iter()
            .map(|date| date.day())
            .collect::<Vec<_>>();
        assert_eq!(days, vec![2, 4]);
        assert!(!result.limited);
    }

    #[test]
    fn rejects_ambiguous_civil_start() {
        let spec = TemporalSpecV1::LocalDatetime {
            local: chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
                .unwrap()
                .and_hms_opt(1, 30, 0)
                .unwrap(),
            time_zone: "America/Los_Angeles".to_owned(),
        };

        assert!(start_datetime(&spec).is_err());
    }
}

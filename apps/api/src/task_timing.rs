//! Task timing shared by ranking and reminders. Dates are local calendar dates,
//! not fabricated instants at which a plant dies or a financial loss occurs.
use chrono::{DateTime, Days, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Ordinary,
    Serious,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RiskTiming {
    /// An evidenced range, not a second hard deadline. The start is when risk
    /// warrants attention; the optional end preserves the stated uncertainty.
    Window {
        starts_on: NaiveDate,
        ends_on: Option<NaiveDate>,
    },
    /// Relative to this occurrence's soft_due. Never anchored to capture time.
    AfterDue { days: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consequence {
    pub description: String,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<RiskTiming>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceMode {
    Calendar,
    AfterCompletion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRecurrence {
    /// A discriminator keeps imported Todoist rules on their existing path.
    pub kind: String,
    pub mode: RecurrenceMode,
    pub every_days: u32,
    pub timezone: Tz,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_on: Option<NaiveDate>,
}

impl Consequence {
    pub fn parse(value: &Value) -> Result<Self, String> {
        let parsed: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if parsed.description.trim().is_empty()
            || parsed.description.len() > 1000
            || parsed.description.chars().any(char::is_control)
        {
            return Err(
                "consequence description must contain 1 to 1000 printable characters".into(),
            );
        }
        match parsed.timing {
            Some(RiskTiming::Window {
                starts_on,
                ends_on: Some(end),
            }) if end < starts_on => return Err("consequence window ends before it starts".into()),
            Some(RiskTiming::AfterDue { days }) if days > 3650 => {
                return Err("consequence after_due days must be 0..3650".into());
            }
            _ => {}
        }
        Ok(parsed)
    }
}

impl NativeRecurrence {
    pub fn parse(value: &Value) -> Result<Option<Self>, String> {
        if value.get("kind").and_then(Value::as_str) != Some("native") {
            return Ok(None);
        }
        let parsed: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if !(1..=3650).contains(&parsed.every_days) {
            return Err("native recurrence every_days must be 1..3650".into());
        }
        match (parsed.mode, parsed.anchor_on) {
            (RecurrenceMode::Calendar, None) => {
                return Err("calendar recurrence requires anchor_on".into());
            }
            (RecurrenceMode::AfterCompletion, Some(_)) => {
                return Err("after_completion uses actual completion, not anchor_on".into());
            }
            _ => {}
        }
        Ok(Some(parsed))
    }

    pub fn next_due(
        &self,
        current_due: Option<NaiveDate>,
        completed_at: DateTime<Utc>,
    ) -> Option<NaiveDate> {
        let completed_on = completed_at.with_timezone(&self.timezone).date_naive();
        match self.mode {
            RecurrenceMode::AfterCompletion => {
                completed_on.checked_add_days(Days::new(self.every_days.into()))
            }
            RecurrenceMode::Calendar => {
                let anchor = self.anchor_on?;
                // Early completion advances beyond the current occurrence;
                // late completion skips missed slots, never generating a flood.
                let after = current_due.map_or(completed_on, |due| due.max(completed_on));
                let elapsed = after.signed_duration_since(anchor).num_days();
                let steps = if elapsed < 0 {
                    0
                } else {
                    elapsed / i64::from(self.every_days) + 1
                };
                anchor.checked_add_days(Days::new(
                    (steps as u64).checked_mul(self.every_days.into())?,
                ))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTiming {
    pub consequence: Option<Consequence>,
    pub recurrence: Option<NativeRecurrence>,
    pub due_on: Option<NaiveDate>,
    pub timezone: Tz,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimingAttention {
    pub needs_attention: bool,
    pub serious: bool,
    pub timing_unknown: bool,
    pub risk_on: Option<NaiveDate>,
    pub due_on: Option<NaiveDate>,
    pub reason: String,
}

impl TaskTiming {
    pub fn timezone(&self) -> Tz {
        self.recurrence
            .as_ref()
            .map_or(self.timezone, |r| r.timezone)
    }

    pub fn risk_on(&self) -> Option<NaiveDate> {
        match self.consequence.as_ref()?.timing {
            Some(RiskTiming::Window { starts_on, .. }) => Some(starts_on),
            Some(RiskTiming::AfterDue { days }) => {
                self.due_on?.checked_add_days(Days::new(days.into()))
            }
            None => None,
        }
    }

    pub fn attention(&self, as_of: DateTime<Utc>) -> TimingAttention {
        let today = as_of.with_timezone(&self.timezone()).date_naive();
        let risk_on = self.risk_on();
        let risk_active = risk_on.is_some_and(|date| date <= today);
        let routine_due =
            self.recurrence.is_some() && self.due_on.is_some_and(|date| date <= today);
        let serious = risk_active
            && self
                .consequence
                .as_ref()
                .is_some_and(|c| c.severity == Severity::Serious);
        let reason = if risk_active {
            format!(
                "{} · {}",
                if serious {
                    "Serious consequence"
                } else {
                    "Timing-sensitive"
                },
                self.consequence
                    .as_ref()
                    .expect("risk has consequence")
                    .description
            )
        } else if routine_due {
            format!(
                "Routine {}",
                if self.due_on == Some(today) {
                    "due today"
                } else {
                    "overdue"
                }
            )
        } else if let Some(date) = risk_on {
            format!("Risk window starts {date}")
        } else if let Some(date) = self.due_on.filter(|_| self.recurrence.is_some()) {
            format!("Routine due {date}")
        } else {
            "Timing needs clarification — no reminder scheduled".into()
        };
        TimingAttention {
            needs_attention: risk_active || routine_due,
            serious,
            timing_unknown: risk_on.is_none()
                && (self.recurrence.is_none() || self.due_on.is_none()),
            risk_on,
            due_on: self.due_on,
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn native_recurrence_uses_actual_completion_and_calendar_skips_missed_slots() {
        let completed = Utc.with_ymd_and_hms(2026, 9, 13, 2, 0, 0).unwrap();
        let current = NaiveDate::from_ymd_opt(2026, 9, 1);
        let after = NativeRecurrence::parse(&json!({"kind":"native","mode":"after_completion","every_days":14,"timezone":"America/Los_Angeles"})).unwrap().unwrap();
        assert_eq!(
            after.next_due(current, completed).unwrap().to_string(),
            "2026-09-26"
        );
        let calendar = NativeRecurrence::parse(&json!({"kind":"native","mode":"calendar","every_days":7,"timezone":"America/Los_Angeles","anchor_on":"2026-09-01"})).unwrap().unwrap();
        assert_eq!(
            calendar.next_due(current, completed).unwrap().to_string(),
            "2026-09-15"
        );
        assert_eq!(
            calendar
                .next_due(NaiveDate::from_ymd_opt(2026, 9, 15), completed)
                .unwrap()
                .to_string(),
            "2026-09-22"
        );
    }

    #[test]
    fn unknown_timing_is_not_an_invented_deadline_or_risk() {
        let consequence =
            Consequence::parse(&json!({"description":"Plants can die","severity":"serious"}))
                .unwrap();
        let timing = TaskTiming {
            consequence: Some(consequence),
            recurrence: None,
            due_on: None,
            timezone: chrono_tz::UTC,
        };
        let attention = timing.attention(Utc::now());
        assert!(attention.timing_unknown);
        assert!(!attention.needs_attention);
        assert!(!attention.serious);
    }

    #[test]
    fn risk_window_and_relative_tolerance_are_shared_calendar_semantics() {
        let consequence = Consequence::parse(&json!({"description":"Loss of plants","severity":"serious","timing":{"kind":"after_due","days":2}})).unwrap();
        let timing = TaskTiming {
            consequence: Some(consequence),
            recurrence: None,
            due_on: NaiveDate::from_ymd_opt(2026, 9, 10),
            timezone: chrono_tz::America::Los_Angeles,
        };
        assert!(
            !timing
                .attention(Utc.with_ymd_and_hms(2026, 9, 12, 6, 0, 0).unwrap())
                .needs_attention
        );
        assert!(
            timing
                .attention(Utc.with_ymd_and_hms(2026, 9, 12, 7, 0, 0).unwrap())
                .serious
        );
        assert!(Consequence::parse(&json!({"description":"Loss","severity":"serious","timing":{"kind":"window","starts_on":"2026-09-12","ends_on":"2026-09-11"}})).is_err());
        assert!(
            NativeRecurrence::parse(
                &json!({"kind":"native","mode":"after_completion","every_days":0,"timezone":"UTC"})
            )
            .is_err()
        );
        assert!(
            NativeRecurrence::parse(
                &json!({"kind":"native","mode":"calendar","every_days":7,"timezone":"Mars"})
            )
            .is_err()
        );
    }
}

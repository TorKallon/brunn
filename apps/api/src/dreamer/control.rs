//! `dreams/CONTROL.md` — the fail-closed dreaming switch.
//!
//! The file is plain `key: value` lines. A missing file, an unparseable
//! line, an unknown key or value, a duplicate or missing key, or
//! `enabled` ≠ true all mean the same thing: dreaming is disabled and the
//! run exits without writing anything.

use chrono::NaiveDate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    ReportOnly,
    Full,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::ReportOnly => "report-only",
            Mode::Full => "full",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub mode: Mode,
    pub advance_after: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlState {
    Disabled { reason: String },
    Enabled(Control),
}

impl ControlState {
    fn disabled(reason: impl Into<String>) -> Self {
        ControlState::Disabled {
            reason: reason.into(),
        }
    }
}

/// Parse CONTROL.md content. `None` means the file does not exist.
pub fn parse(content: Option<&str>) -> ControlState {
    let Some(content) = content else {
        return ControlState::disabled("CONTROL.md is missing");
    };
    let mut enabled: Option<bool> = None;
    let mut mode: Option<Mode> = None;
    let mut advance_after: Option<NaiveDate> = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return ControlState::disabled(format!("unparseable CONTROL line: {line:?}"));
        };
        let value = value.trim();
        match key.trim() {
            "enabled" => {
                if enabled.is_some() {
                    return ControlState::disabled("duplicate CONTROL key: enabled");
                }
                enabled = match value {
                    "true" => Some(true),
                    "false" => Some(false),
                    other => {
                        return ControlState::disabled(format!("unknown enabled value: {other:?}"));
                    }
                };
            }
            "mode" => {
                if mode.is_some() {
                    return ControlState::disabled("duplicate CONTROL key: mode");
                }
                mode = match value {
                    "report-only" => Some(Mode::ReportOnly),
                    "full" => Some(Mode::Full),
                    other => {
                        return ControlState::disabled(format!("unknown mode value: {other:?}"));
                    }
                };
            }
            "advance_after" => {
                if advance_after.is_some() {
                    return ControlState::disabled("duplicate CONTROL key: advance_after");
                }
                advance_after = match NaiveDate::parse_from_str(value, "%Y-%m-%d") {
                    Ok(date) => Some(date),
                    Err(_) => {
                        return ControlState::disabled(format!(
                            "unparseable advance_after date: {value:?}"
                        ));
                    }
                };
            }
            other => {
                return ControlState::disabled(format!("unknown CONTROL key: {other:?}"));
            }
        }
    }
    let (Some(enabled), Some(mode), Some(advance_after)) = (enabled, mode, advance_after) else {
        return ControlState::disabled("CONTROL is missing a required key");
    };
    if !enabled {
        return ControlState::disabled("CONTROL enabled: false");
    }
    ControlState::Enabled(Control {
        mode,
        advance_after,
    })
}

/// Render CONTROL.md content. Used for the day-8 mode flip and for
/// Pause/Resume rewrites; always emits the strict shape `parse` accepts.
pub fn render(enabled: bool, mode: Mode, advance_after: NaiveDate) -> String {
    format!(
        "enabled: {enabled}\nmode: {}\nadvance_after: {}\n",
        mode.as_str(),
        advance_after.format("%Y-%m-%d"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("test date")
    }

    #[test]
    fn missing_file_is_disabled() {
        assert!(matches!(parse(None), ControlState::Disabled { .. }));
    }

    #[test]
    fn well_formed_enabled_control_parses() {
        let state = parse(Some(
            "enabled: true\nmode: report-only\nadvance_after: 2026-09-05\n",
        ));
        assert_eq!(
            state,
            ControlState::Enabled(Control {
                mode: Mode::ReportOnly,
                advance_after: date("2026-09-05"),
            })
        );
    }

    #[test]
    fn full_mode_parses() {
        let state = parse(Some(
            "enabled: true\nmode: full\nadvance_after: 2026-09-05\n",
        ));
        assert_eq!(
            state,
            ControlState::Enabled(Control {
                mode: Mode::Full,
                advance_after: date("2026-09-05"),
            })
        );
    }

    #[test]
    fn disabled_when_enabled_false() {
        let state = parse(Some(
            "enabled: false\nmode: full\nadvance_after: 2026-09-05\n",
        ));
        assert!(matches!(state, ControlState::Disabled { .. }));
    }

    #[test]
    fn fail_closed_matrix() {
        let cases = [
            // unparseable line
            "enabled true\nmode: full\nadvance_after: 2026-09-05\n",
            // unknown key
            "enabled: true\nmode: full\nadvance_after: 2026-09-05\nextra: yes\n",
            // unknown enabled value
            "enabled: yes\nmode: full\nadvance_after: 2026-09-05\n",
            // unknown mode value
            "enabled: true\nmode: aggressive\nadvance_after: 2026-09-05\n",
            // malformed date
            "enabled: true\nmode: full\nadvance_after: soon\n",
            // missing keys
            "enabled: true\n",
            "mode: full\nadvance_after: 2026-09-05\n",
            // duplicate key
            "enabled: true\nenabled: true\nmode: full\nadvance_after: 2026-09-05\n",
            // empty file
            "",
        ];
        for content in cases {
            assert!(
                matches!(parse(Some(content)), ControlState::Disabled { .. }),
                "expected disabled for {content:?}"
            );
        }
    }

    #[test]
    fn render_round_trips() {
        let rendered = render(true, Mode::ReportOnly, date("2026-09-05"));
        assert_eq!(
            parse(Some(&rendered)),
            ControlState::Enabled(Control {
                mode: Mode::ReportOnly,
                advance_after: date("2026-09-05"),
            })
        );
        let paused = render(false, Mode::ReportOnly, date("2026-09-05"));
        assert!(matches!(
            parse(Some(&paused)),
            ControlState::Disabled { .. }
        ));
    }
}

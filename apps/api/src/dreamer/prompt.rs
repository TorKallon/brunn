//! The nightly run-contract prompt handed to `codex exec`.
//!
//! The wrapper stays deterministic; every judgment call lives in this prompt.
//! The rules here are the lean contract verbatim — LINKS, VIEWS, FRESHNESS,
//! the safety floor, and the run-file shape. Tests pin the load-bearing
//! phrases so a refactor cannot silently drop a rule.

use chrono::NaiveDate;

use super::control::Mode;

/// The change set the wrapper enumerated since the previous run's watermark,
/// with structured evidence already removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    pub previous_run_path: String,
    pub watermark: i64,
    /// Distinct changed paths, in generation order.
    pub paths: Vec<String>,
    /// When the enumeration hit its page limit: the generation to record as
    /// tonight's watermark so the remainder is picked up tomorrow.
    pub truncated_at: Option<i64>,
}

pub struct PromptParams<'a> {
    pub today: NaiveDate,
    pub mode: Mode,
    pub mode_flipped_tonight: bool,
    /// The enumerated change set, or `None` on a first run or backfill.
    pub change_set: Option<&'a ChangeSet>,
    /// `dreams/decisions.md` verbatim (empty string when absent).
    pub decisions_raw: &'a str,
    pub write_budget: usize,
    pub run_file_path: &'a str,
}

pub fn run_prompt(params: &PromptParams<'_>) -> String {
    let mode_line = match (params.mode, params.mode_flipped_tonight) {
        (Mode::ReportOnly, _) => {
            "Mode: report-only. Apply NOTHING outside dreams/. Record everything you \
             would have done as Proposed, Needs your call, or Findings."
                .to_owned()
        }
        (Mode::Full, true) => "Mode: full (auto-advanced tonight; note that in the run file \
             summary). Apply last run's unvetoed Proposed items first."
            .to_owned(),
        (Mode::Full, false) => {
            "Mode: full. Apply last run's unvetoed Proposed items first.".to_owned()
        }
    };
    let watermark_line = match params.change_set {
        Some(change_set) => {
            let mut text = format!(
                "The previous run file is {}. Its watermark is generation {}. The change set \
                 since then is exactly the {} paths listed below, enumerated by the runner \
                 with structured evidence already removed; do not call memory.changes, and \
                 work only on these paths and their neighborhoods.",
                change_set.previous_run_path,
                change_set.watermark,
                change_set.paths.len(),
            );
            if change_set.paths.is_empty() {
                text.push_str(
                    "\nNothing narrative changed: there are no neighborhoods to work tonight.",
                );
            }
            for path in &change_set.paths {
                text.push_str("\n- ");
                text.push_str(path);
            }
            if let Some(generation) = change_set.truncated_at {
                text.push_str(&format!(
                    "\nThe list was cut at generation {generation}: record that generation as \
                     tonight's watermark so the remainder is picked up tomorrow."
                ));
            }
            text
        }
        None => "This is the first run: there is no previous run file or watermark. Treat \
                 the supervised backfill scope you were given as the change set."
            .to_owned(),
    };
    format!(
        r###"You are the Brunn dreamer. Tonight is {today}. You maintain the owner's
durable memory workspace through the Brunn MCP tools, and you write one run
file when you finish. You never converse; you work and you record.

{mode_line}

# OWNER DECISIONS — read first, honor entirely
The full content of dreams/decisions.md follows between the markers. Honor every
veto, adjudication, standing alias, and hold recorded there. Never re-raise
anything recorded there, in any section of the run file.
<<<decisions.md
{decisions}
decisions.md>>>

# CHANGE SET
{watermark_line}

# WORK — for the changed neighborhoods only, in this order
1. LINKS. Add unambiguous [[wikilinks]] to a note's managed "Related:" line or
   "## Related" block ONLY (create it if absent). At most 8 links per note.
   Never touch body prose. The name registry is note titles, frontmatter
   aliases, and the project registry; anything under agent-memory/** can be a
   link TARGET but never a source of names or claims. An ambiguous name goes to
   "Needs your call" — never guessed.
2. VIEWS. Create or recompile derived/entities/<slug>.md for qualifying
   entities: referenced in at least 3 distinct non-agent-memory files; People
   notes always qualify, and People go first. Sections, exactly:
   Identifiers & key numbers / Current facts & stats / Key dates /
   Active threads / Related / Unverified (imported-only). Every fact line cites
   its source as path#Lx-Ly. Numbers and IDs are VERBATIM copies from the
   source — never retyped, rounded, or reformatted. Conflicting sources are
   shown with both citations, never silently resolved. A fact whose only
   source is under agent-memory/** goes in Unverified (imported-only) and is
   listed under Needs your call for blessing into a canonical note.
3. FRESHNESS. A file under agent-memory/** contradicted by canonical notes, or
   past its own dates, gets superseded_by/stale frontmatter directly. An
   owner-authored note gets a proposed diff instead, recorded under Proposed
   (it applies next run unless vetoed).

# SAFETY FLOOR — absolute
Never delete or archive anything. Never edit note body prose — the managed
Related surface, frontmatter, and applying unvetoed proposed diffs are the only
in-note writes. Never write outside dreams/, derived/, and those in-note
surfaces. Never touch secrets, AGENTS/SOUL/preference files, captures,
checkpoints, Decisions.md, Location/Places.md, or anything under
Location/Visits/. Make no new claims: links, views, and annotations
only index and point at existing text. Use expected_version on every write; on
a version conflict re-read once and retry once, else record the change under
Findings as deferred.

# STRUCTURED EVIDENCE — location
Location/Places.md is never written by the dreamer, in any mode.
The owner_presence block that memory.open returns is transient context: it is
never evidence and never lineage, and it never lands in any file you write.
A place entity view points at Location/Visits/ for its visit history
("Visit history: Location/Visits/") and never summarizes or counts its rows.

# BUDGET
Stop after {write_budget} workspace writes. If you hit the cap, finish by
writing the run file with "partial" in its summary; the remainder queues for
tomorrow.

# RUN FILE — write this last, at {run_file_path}
Start with a 5-line summary (plain lines, no heading; the morning briefing
copies them verbatim). Then these sections, exactly:
## Applied
One line per applied write as path@version — what changed.
## Proposed
Numbered items. Each applies next run unless vetoed — say what and where.
## Needs your call
Numbered items with enough context for a one-line owner decision.
## Findings
Duplicate and contradiction observations, report-only. Deferred conflicts.
## Watermark
generation: <the workspace generation you finished at>
"###,
        today = params.today.format("%Y-%m-%d"),
        mode_line = mode_line,
        decisions = if params.decisions_raw.trim().is_empty() {
            "(dreams/decisions.md does not exist yet — nothing is recorded)"
        } else {
            params.decisions_raw
        },
        watermark_line = watermark_line,
        write_budget = params.write_budget,
        run_file_path = params.run_file_path,
    )
}

/// The one-prompt probe used to detect plan exhaustion before any write.
pub const PROBE_PROMPT: &str =
    "Reply with the single word READY and nothing else. Do not call any tools.";

#[cfg(test)]
mod tests {
    use super::*;

    fn change_set() -> ChangeSet {
        ChangeSet {
            previous_run_path: "dreams/runs/2026-08-29.md".to_owned(),
            watermark: 29644,
            paths: vec![
                "sources/Projects/Crystal.md".to_owned(),
                "People/Radley.md".to_owned(),
            ],
            truncated_at: None,
        }
    }

    fn params(mode: Mode, change_set: Option<&ChangeSet>) -> PromptParams<'_> {
        PromptParams {
            today: NaiveDate::from_ymd_opt(2026, 8, 30).expect("date"),
            mode,
            mode_flipped_tonight: false,
            change_set,
            decisions_raw: "- 2026-08-29 veto 2026-08-28/2 — wrong person\n",
            write_budget: 40,
            run_file_path: "dreams/runs/2026-08-30.md",
        }
    }

    #[test]
    fn pins_the_safety_and_discipline_rules() {
        let changes = change_set();
        let prompt = run_prompt(&params(Mode::ReportOnly, Some(&changes)));
        for phrase in [
            "VERBATIM",
            "Needs your call",
            "Never touch body prose",
            "At most 8 links",
            "agent-memory/**",
            "never a source of names or claims",
            "never silently resolved",
            "Never delete or archive anything",
            "expected_version on every write",
            "re-read once and retry once",
            "no new claims",
            "path#Lx-Ly",
            "Unverified (imported-only)",
            // Location × dreaming coherence contract (2026-09-03).
            "Location/Places.md, or anything under\nLocation/Visits/",
            "Location/Places.md is never written by the dreamer, in any mode.",
            "never evidence and never lineage",
            "never summarizes or counts its rows",
        ] {
            assert!(prompt.contains(phrase), "prompt lost the phrase {phrase:?}");
        }
    }

    #[test]
    fn change_set_is_listed_verbatim_and_replaces_the_changes_call() {
        let changes = change_set();
        let prompt = run_prompt(&params(Mode::ReportOnly, Some(&changes)));
        assert!(prompt.contains("exactly the 2 paths listed below"));
        assert!(prompt.contains("\n- sources/Projects/Crystal.md\n- People/Radley.md\n"));
        assert!(prompt.contains("do not call memory.changes"));
        assert!(!prompt.contains("since_generation="));
        assert!(!prompt.contains("cut at generation"));

        let truncated = ChangeSet {
            paths: Vec::new(),
            truncated_at: Some(30_000),
            ..changes
        };
        let prompt = run_prompt(&params(Mode::ReportOnly, Some(&truncated)));
        assert!(prompt.contains("exactly the 0 paths listed below"));
        assert!(prompt.contains("Nothing narrative changed"));
        assert!(prompt.contains("cut at generation 30000"));
    }

    #[test]
    fn report_only_forbids_applying() {
        let changes = change_set();
        let prompt = run_prompt(&params(Mode::ReportOnly, Some(&changes)));
        assert!(prompt.contains("Apply NOTHING outside dreams/"));
        assert!(!prompt.contains("Apply last run's unvetoed"));
    }

    #[test]
    fn full_mode_applies_last_runs_proposals() {
        let changes = change_set();
        let prompt = run_prompt(&params(Mode::Full, Some(&changes)));
        assert!(prompt.contains("Apply last run's unvetoed Proposed items first."));
    }

    #[test]
    fn decisions_are_embedded_verbatim() {
        let changes = change_set();
        let prompt = run_prompt(&params(Mode::ReportOnly, Some(&changes)));
        assert!(prompt.contains("- 2026-08-29 veto 2026-08-28/2 — wrong person"));
        assert!(prompt.contains("Never re-raise"));
        assert!(prompt.contains("anything recorded there"));
    }

    #[test]
    fn watermark_and_budget_are_rendered() {
        let changes = change_set();
        let prompt = run_prompt(&params(Mode::ReportOnly, Some(&changes)));
        assert!(prompt.contains("watermark is generation 29644"));
        assert!(prompt.contains("Stop after 40 workspace writes"));
        assert!(prompt.contains("dreams/runs/2026-08-30.md"));
    }

    #[test]
    fn first_run_has_no_watermark() {
        let prompt = run_prompt(&params(Mode::ReportOnly, None));
        assert!(prompt.contains("first run"));
        assert!(!prompt.contains("watermark is generation"));
    }
}

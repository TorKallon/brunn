//! The nightly run-contract prompt handed to `codex exec`.
//!
//! The wrapper stays deterministic; every judgment call lives in this prompt.
//! The rules here are the lean contract verbatim — LINKS, VIEWS, FRESHNESS,
//! the safety floor, and the run-file shape. Tests pin the load-bearing
//! phrases so a refactor cannot silently drop a rule.

use chrono::NaiveDate;

use super::control::Mode;

pub struct PromptParams<'a> {
    pub today: NaiveDate,
    pub mode: Mode,
    pub mode_flipped_tonight: bool,
    /// Path and parsed watermark of the previous run file, if any.
    pub last_run: Option<(&'a str, i64)>,
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
    let watermark_line = match params.last_run {
        Some((path, watermark)) => format!(
            "The previous run file is {path}. Its watermark is generation {watermark}: \
             call memory.changes with since_generation={watermark} and work only on those \
             changed neighborhoods."
        ),
        None => "This is the first run: there is no previous run file or watermark. Treat \
                 the supervised backfill scope you were given as the change set."
            .to_owned(),
    };
    format!(
        r###"You are the Straylight dreamer. Tonight is {today}. You maintain the owner's
durable memory workspace through the Straylight MCP tools, and you write one run
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
checkpoints, or Decisions.md. Make no new claims: links, views, and annotations
only index and point at existing text. Use expected_version on every write; on
a version conflict re-read once and retry once, else record the change under
Findings as deferred.

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

    fn params(mode: Mode) -> PromptParams<'static> {
        PromptParams {
            today: NaiveDate::from_ymd_opt(2026, 8, 30).expect("date"),
            mode,
            mode_flipped_tonight: false,
            last_run: Some(("dreams/runs/2026-08-29.md", 29644)),
            decisions_raw: "- 2026-08-29 veto 2026-08-28/2 — wrong person\n",
            write_budget: 40,
            run_file_path: "dreams/runs/2026-08-30.md",
        }
    }

    #[test]
    fn pins_the_safety_and_discipline_rules() {
        let prompt = run_prompt(&params(Mode::ReportOnly));
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
        ] {
            assert!(prompt.contains(phrase), "prompt lost the phrase {phrase:?}");
        }
    }

    #[test]
    fn report_only_forbids_applying() {
        let prompt = run_prompt(&params(Mode::ReportOnly));
        assert!(prompt.contains("Apply NOTHING outside dreams/"));
        assert!(!prompt.contains("Apply last run's unvetoed"));
    }

    #[test]
    fn full_mode_applies_last_runs_proposals() {
        let prompt = run_prompt(&params(Mode::Full));
        assert!(prompt.contains("Apply last run's unvetoed Proposed items first."));
    }

    #[test]
    fn decisions_are_embedded_verbatim() {
        let prompt = run_prompt(&params(Mode::ReportOnly));
        assert!(prompt.contains("- 2026-08-29 veto 2026-08-28/2 — wrong person"));
        assert!(prompt.contains("Never re-raise"));
        assert!(prompt.contains("anything recorded there"));
    }

    #[test]
    fn watermark_and_budget_are_rendered() {
        let prompt = run_prompt(&params(Mode::ReportOnly));
        assert!(prompt.contains("since_generation=29644"));
        assert!(prompt.contains("Stop after 40 workspace writes"));
        assert!(prompt.contains("dreams/runs/2026-08-30.md"));
    }

    #[test]
    fn first_run_has_no_watermark() {
        let mut p = params(Mode::ReportOnly);
        p.last_run = None;
        let prompt = run_prompt(&p);
        assert!(prompt.contains("first run"));
        assert!(!prompt.contains("since_generation="));
    }
}

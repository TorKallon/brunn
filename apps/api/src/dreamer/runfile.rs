//! `dreams/runs/YYYY-MM-DD.md` — one file per executed run; the entire audit
//! trail and revert map.
//!
//! Codex writes the real run file last, through MCP. The wrapper only ever
//! needs to: locate and skim a run file (summary, watermark, applied paths),
//! and write a minimal `failed` one when codex dies without writing its own.

use chrono::NaiveDate;

pub fn run_path(date: NaiveDate) -> String {
    format!("dreams/runs/{}.md", date.format("%Y-%m-%d"))
}

/// The briefing copies the first five non-empty, non-heading lines.
pub fn summary_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .take(5)
        .map(str::to_owned)
        .collect()
}

/// `path@version` entries under `## Applied`.
pub fn applied_paths(content: &str) -> Vec<(String, i64)> {
    section_lines(content, "Applied")
        .iter()
        .filter_map(|line| {
            let token = line
                .trim_start_matches(['-', '*'])
                .split_whitespace()
                .next()?;
            let token = token
                .trim_end_matches(['`', ',', ';'])
                .trim_start_matches('`');
            let (path, version) = token.rsplit_once('@')?;
            Some((path.to_owned(), version.parse::<i64>().ok()?))
        })
        .collect()
}

/// Items under `## Needs your call`, with their 1-based numbers.
pub fn needs_your_call(content: &str) -> Vec<String> {
    section_lines(content, "Needs your call")
        .iter()
        .filter(|line| {
            line.starts_with(['-', '*']) || line.chars().next().is_some_and(|c| c.is_ascii_digit())
        })
        .map(|line| (*line).to_owned())
        .collect()
}

/// The `memory.changes` generation recorded under `## Watermark`.
pub fn watermark(content: &str) -> Option<i64> {
    section_lines(content, "Watermark").iter().find_map(|line| {
        line.split(|c: char| !c.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .map(str::parse::<i64>)
            .find_map(Result::ok)
    })
}

fn section_lines<'a>(content: &'a str, heading: &str) -> Vec<&'a str> {
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix("##") {
            in_section = title.trim().eq_ignore_ascii_case(heading);
            continue;
        }
        if in_section && !trimmed.is_empty() {
            lines.push(trimmed);
        }
    }
    lines
}

/// The minimal run file the wrapper writes when codex died (or never started)
/// after the run had already begun. Keeps the audit trail unbroken without
/// inventing content.
pub fn fallback_run_file(date: NaiveDate, status: &str, detail: &str, watermark: i64) -> String {
    format!(
        concat!(
            "# Dreaming run {date}\n\n",
            "Status: {status}.\n",
            "{detail}\n",
            "No proposals were recorded by this run.\n",
            "Nothing was applied by this run.\n",
            "The next run resumes from the watermark below.\n\n",
            "## Applied\n\n",
            "## Proposed\n\n",
            "## Needs your call\n\n",
            "## Findings\n\n",
            "## Watermark\n\n",
            "generation: {watermark}\n",
        ),
        date = date.format("%Y-%m-%d"),
        status = status,
        detail = detail,
        watermark = watermark,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = concat!(
        "# Dreaming run 2026-08-30\n",
        "\n",
        "Linked 4 notes and recompiled 2 entity views.\n",
        "Proposed 1 freshness diff on an owner note.\n",
        "2 items need your call.\n",
        "No contradictions were found.\n",
        "Budget: 11 of 40 writes used.\n",
        "\n",
        "## Applied\n",
        "- sources/Projects/Metis/Notes.md@7 — Related line\n",
        "- `derived/entities/radley.md@3` — recompiled\n",
        "\n",
        "## Proposed\n",
        "1. Freshness diff for sources/Health/Weight.md\n",
        "\n",
        "## Needs your call\n",
        "1. \"Jen\" is ambiguous between [[People/Jen]] and [[People/Jenny]]\n",
        "2. Bless agent-memory fact into a canonical note\n",
        "\n",
        "## Findings\n",
        "- none\n",
        "\n",
        "## Watermark\n",
        "generation: 29644\n",
    );

    #[test]
    fn parses_summary_applied_calls_and_watermark() {
        assert_eq!(summary_lines(SAMPLE).len(), 5);
        assert_eq!(
            summary_lines(SAMPLE)[0],
            "Linked 4 notes and recompiled 2 entity views."
        );
        assert_eq!(
            applied_paths(SAMPLE),
            vec![
                ("sources/Projects/Metis/Notes.md".to_owned(), 7),
                ("derived/entities/radley.md".to_owned(), 3),
            ]
        );
        assert_eq!(needs_your_call(SAMPLE).len(), 2);
        assert_eq!(watermark(SAMPLE), Some(29644));
    }

    #[test]
    fn run_path_shape() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 30).expect("date");
        assert_eq!(run_path(date), "dreams/runs/2026-08-30.md");
    }

    #[test]
    fn fallback_run_file_round_trips() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 30).expect("date");
        let content = fallback_run_file(date, "failed", "codex exited early.", 29644);
        assert_eq!(watermark(&content), Some(29644));
        assert!(applied_paths(&content).is_empty());
        assert!(needs_your_call(&content).is_empty());
        let summary = summary_lines(&content);
        assert_eq!(summary.len(), 5);
        assert_eq!(summary[0], "Status: failed.");
    }

    #[test]
    fn missing_sections_parse_to_nothing() {
        assert_eq!(watermark("# empty\n"), None);
        assert!(applied_paths("# empty\n").is_empty());
        assert!(summary_lines("").is_empty());
    }
}

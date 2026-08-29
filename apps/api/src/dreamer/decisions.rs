//! `dreams/decisions.md` — the append-only owner record.
//!
//! Agents write the strict line grammar; the dreamer reads leniently. The
//! wrapper only needs three signals — vetoes (to withhold Proposed items),
//! `hold-advance` (to block the mode flip), and the raw text (fed to the
//! dreaming prompt in full so nothing recorded is ever re-raised).

use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decisions {
    /// Veto targets in `<run-date>/<item-n>` form, e.g. `2026-08-30/3`.
    pub vetoes: BTreeSet<String>,
    /// Standing aliases: (name, wikilink target).
    pub aliases: Vec<(String, String)>,
    pub hold_advance: bool,
    /// The full file text, verbatim, for the dreaming prompt.
    pub raw: String,
}

/// Lenient parse: a line contributes a signal if it plausibly carries one;
/// nothing about the file can fail the run.
pub fn parse(content: &str) -> Decisions {
    let mut decisions = Decisions {
        raw: content.to_owned(),
        ..Decisions::default()
    };
    for line in content.lines() {
        let line = line.trim().trim_start_matches('-').trim();
        if line.is_empty() {
            continue;
        }
        if line.contains("hold-advance") {
            decisions.hold_advance = true;
        }
        if let Some(rest) = split_after_keyword(line, "veto") {
            if let Some(target) = rest.split_whitespace().find(|token| is_veto_target(token)) {
                decisions
                    .vetoes
                    .insert(target.trim_end_matches(['.', ',', ';']).to_owned());
            }
        }
        if let Some(rest) = split_after_keyword(line, "alias") {
            if let Some(alias) = parse_alias(rest) {
                decisions.aliases.push(alias);
            }
        }
    }
    decisions
}

/// Find ` keyword ` as a standalone word and return the remainder of the line.
fn split_after_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let mut search_from = 0;
    while let Some(offset) = line[search_from..].find(keyword) {
        let start = search_from + offset;
        let end = start + keyword.len();
        let before_ok = start == 0
            || line[..start]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_alphanumeric());
        let after_ok = line[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return Some(&line[end..]);
        }
        search_from = end;
    }
    None
}

/// `<run-date>/<item-n>`: a YYYY-MM-DD date, a slash, and an item token.
fn is_veto_target(token: &str) -> bool {
    let Some((date, item)) = token.split_once('/') else {
        return false;
    };
    let date_ok = date.len() == 10
        && date.chars().enumerate().all(|(i, c)| match i {
            4 | 7 => c == '-',
            _ => c.is_ascii_digit(),
        });
    let item = item.trim_end_matches(['.', ',', ';']);
    let item_number = item.strip_prefix("item-").unwrap_or(item);
    date_ok && !item_number.is_empty() && item_number.chars().all(|c| c.is_ascii_digit())
}

/// `"<name>" = [[Target]]`
fn parse_alias(rest: &str) -> Option<(String, String)> {
    let rest = rest.trim();
    let quoted = rest.strip_prefix('"')?;
    let (name, after_name) = quoted.split_once('"')?;
    let after_eq = after_name.trim_start().strip_prefix('=')?.trim_start();
    let target = after_eq.strip_prefix("[[")?;
    let (target, _) = target.split_once("]]")?;
    if name.is_empty() || target.is_empty() {
        return None;
    }
    Some((name.to_owned(), target.to_owned()))
}

/// True when a Proposed item from `run_date` with 1-based index `item_number`
/// has been vetoed. Both `2026-08-30/3` and `2026-08-30/item-3` count.
pub fn is_vetoed(decisions: &Decisions, run_date: &str, item_number: usize) -> bool {
    decisions
        .vetoes
        .contains(&format!("{run_date}/{item_number}"))
        || decisions
            .vetoes
            .contains(&format!("{run_date}/item-{item_number}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_grammar_lines() {
        let decisions = parse(concat!(
            "- 2026-08-31 veto 2026-08-30/2 — wrong person\n",
            "- 2026-08-31 alias \"Radley\" = [[People/Radley Metzger]]\n",
            "- 2026-09-01 adjudicate weight units — 42.18 is current\n",
            "- 2026-09-02 hold-advance\n",
        ));
        assert!(decisions.vetoes.contains("2026-08-30/2"));
        assert_eq!(
            decisions.aliases,
            vec![("Radley".to_owned(), "People/Radley Metzger".to_owned())]
        );
        assert!(decisions.hold_advance);
        assert!(is_vetoed(&decisions, "2026-08-30", 2));
        assert!(!is_vetoed(&decisions, "2026-08-30", 1));
    }

    #[test]
    fn reads_leniently() {
        let decisions = parse(concat!(
            "vetoed: please veto 2026-08-30/item-4, it is wrong\n",
            "Alias \"Jen\"=[[People/Jen]] (standing)\n",
            "we should hold-advance for now\n",
        ));
        assert!(decisions.vetoes.contains("2026-08-30/item-4"));
        assert!(is_vetoed(&decisions, "2026-08-30", 4));
        assert!(decisions.hold_advance);
        // "Alias" (capitalized) is not the keyword; lenient but not fuzzy.
        assert!(decisions.aliases.is_empty());
    }

    #[test]
    fn empty_and_prose_lines_contribute_nothing() {
        let decisions = parse("Just some notes.\n\n- 2026-09-01 looked fine\n");
        assert!(decisions.vetoes.is_empty());
        assert!(decisions.aliases.is_empty());
        assert!(!decisions.hold_advance);
    }

    #[test]
    fn veto_requires_a_dated_target() {
        let decisions = parse("- 2026-08-31 veto everything\n");
        assert!(decisions.vetoes.is_empty());
    }

    #[test]
    fn raw_text_is_preserved_verbatim() {
        let text = "- 2026-09-02 adjudicate x — y\n";
        assert_eq!(parse(text).raw, text);
    }
}

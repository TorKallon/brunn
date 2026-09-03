//! The change set handed to codex, and the dreamer's write gate.
//!
//! One predicate, applied once: files whose frontmatter `kind` marks them as
//! structured evidence never enter the change set, so they never reach the
//! LINKS sweep, VIEWS candidacy or compile inputs, FRESHNESS, or the choice
//! of which neighborhoods a run works on.

/// Frontmatter kinds that are structured evidence, not narrative: the
/// location engine's derived month files and the owner's known places.
pub fn is_structured_evidence(frontmatter_kind: Option<&str>) -> bool {
    matches!(
        frontmatter_kind,
        Some("location-visits" | "location-places")
    )
}

/// The `kind:` value of a leading `---` frontmatter block, if any.
pub fn frontmatter_kind(text: &str) -> Option<&str> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            return None;
        }
        if let Some(value) = line.strip_prefix("kind:") {
            return Some(value.trim());
        }
    }
    None
}

/// Paths the dreamer may never write, in any mode, even when a run file
/// enumerates them: inputs to the deterministic location engine and its
/// derived history.
pub fn write_denied(path: &str) -> bool {
    path == "Location/Places.md" || path.starts_with("Location/Visits/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_kinds_are_structured_evidence_and_nothing_else_is() {
        assert!(is_structured_evidence(Some("location-visits")));
        assert!(is_structured_evidence(Some("location-places")));
        assert!(!is_structured_evidence(Some("human_document")));
        assert!(!is_structured_evidence(Some("")));
        assert!(!is_structured_evidence(None));
    }

    #[test]
    fn frontmatter_kind_reads_only_a_leading_block() {
        assert_eq!(
            frontmatter_kind("---\nkind: location-visits\nmonth: 2026-09\n---\n| a |\n"),
            Some("location-visits")
        );
        assert_eq!(
            frontmatter_kind("---\ntitle: Places\nkind:   location-places\n---\n"),
            Some("location-places")
        );
        assert_eq!(frontmatter_kind("# Note\n\nkind: location-visits\n"), None);
        assert_eq!(
            frontmatter_kind("---\ntitle: x\n---\nkind: location-visits\n"),
            None
        );
        assert_eq!(frontmatter_kind(""), None);
    }

    #[test]
    fn write_gate_rejects_places_and_every_visits_file() {
        assert!(write_denied("Location/Places.md"));
        assert!(write_denied("Location/Visits/2026-09.md"));
        assert!(write_denied("Location/Visits/2027-01.md"));
        assert!(!write_denied("derived/entities/crystal-mountain.md"));
        assert!(!write_denied("dreams/runs/2026-09-03.md"));
        assert!(!write_denied("Location/Notes.md"));
    }
}

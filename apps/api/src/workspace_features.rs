use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use chrono::{NaiveDate, Utc};
use serde::Serialize;
use tokio::sync::RwLock;
use uuid::Uuid;

pub const FRONTMATTER_LIMIT_BYTES: usize = 4 * 1024;
pub const CURRENT_TRUTH_DEPTH_CAP: usize = 8;
pub const PENDING_INTENTION_LIMIT: usize = 5;
pub const PENDING_INTENTION_CHAR_BUDGET: usize = 500;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DerivedFrontmatter {
    pub supersedes: Vec<String>,
    pub kind: Option<String>,
    pub triggers: Vec<String>,
    pub due: Option<String>,
    pub status: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceFeatureDocument {
    pub entry_id: Uuid,
    pub path: String,
    pub title: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SupersessionAnnotation {
    pub path: String,
    pub head_entry: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SupersessionChainItem {
    pub path: String,
    pub entry_ref: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SupersessionWarning {
    pub kind: String,
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PendingIntention {
    pub path: String,
    pub title_line: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    pub status_note: String,
    pub matched_terms: Vec<String>,
}

#[derive(Clone, Debug)]
struct FeatureEntry {
    entry_id: Uuid,
    path: String,
}

#[derive(Clone, Debug)]
struct IntentionEntry {
    path: String,
    title_line: String,
    due: Option<String>,
    triggers: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CurrentTruthResolution {
    pub head_path: String,
    pub chain: Vec<SupersessionChainItem>,
    pub warning: Option<SupersessionWarning>,
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceFeatureSnapshot {
    pub generation: i64,
    entries: HashMap<String, FeatureEntry>,
    direct_successors: HashMap<String, FeatureEntry>,
    pending_intentions: Vec<IntentionEntry>,
}

impl WorkspaceFeatureSnapshot {
    pub fn build(generation: i64, documents: Vec<WorkspaceFeatureDocument>) -> Self {
        let entries = documents
            .iter()
            .map(|document| {
                (
                    document.path.clone(),
                    FeatureEntry {
                        entry_id: document.entry_id,
                        path: document.path.clone(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        let mut edges = Vec::new();
        let mut pending_intentions = Vec::new();
        for document in &documents {
            let fields = parse_frontmatter(&document.content);
            for target in fields.supersedes {
                if entries.contains_key(&target) && target != document.path {
                    edges.push((target, document.path.clone()));
                }
            }
            if fields.kind.as_deref() == Some("intention")
                && fields.status.as_deref() == Some("pending")
                && !fields.triggers.is_empty()
            {
                pending_intentions.push(IntentionEntry {
                    path: document.path.clone(),
                    title_line: title_line(&document.content, &document.title),
                    due: fields.due,
                    triggers: fields.triggers,
                });
            }
        }
        edges.sort();
        let mut direct_successors = HashMap::new();
        for (target, successor) in edges {
            if let Some(entry) = entries.get(&successor) {
                direct_successors
                    .entry(target)
                    .or_insert_with(|| entry.clone());
            }
        }
        pending_intentions.sort_by(|left, right| left.path.cmp(&right.path));
        Self {
            generation,
            entries,
            direct_successors,
            pending_intentions,
        }
    }

    pub fn contains_path(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    pub fn superseded_by(&self, path: &str) -> Option<SupersessionAnnotation> {
        let immediate = self.direct_successors.get(path)?;
        let resolution = self.resolve_current_truth(path);
        let head = self.entries.get(&resolution.head_path).unwrap_or(immediate);
        Some(SupersessionAnnotation {
            path: immediate.path.clone(),
            head_entry: format!("entry:{}", head.entry_id),
        })
    }

    pub fn resolve_current_truth(&self, path: &str) -> CurrentTruthResolution {
        let mut current = path.to_owned();
        let mut seen = HashSet::from([current.clone()]);
        let mut chain = self
            .entries
            .get(path)
            .map(chain_item)
            .into_iter()
            .collect::<Vec<_>>();

        for _ in 0..CURRENT_TRUTH_DEPTH_CAP {
            let Some(next) = self.direct_successors.get(&current) else {
                return CurrentTruthResolution {
                    head_path: current,
                    chain,
                    warning: None,
                };
            };
            chain.push(chain_item(next));
            if !seen.insert(next.path.clone()) {
                return CurrentTruthResolution {
                    head_path: path.to_owned(),
                    chain,
                    warning: Some(SupersessionWarning {
                        kind: "supersession_cycle".to_owned(),
                        path: path.to_owned(),
                        message: "the authored supersedes chain contains a cycle; the requested document was returned"
                            .to_owned(),
                    }),
                };
            }
            current.clone_from(&next.path);
        }

        if self.direct_successors.contains_key(&current) {
            CurrentTruthResolution {
                head_path: path.to_owned(),
                chain,
                warning: Some(SupersessionWarning {
                    kind: "supersession_depth_cap".to_owned(),
                    path: path.to_owned(),
                    message: format!(
                        "the authored supersedes chain exceeded the depth cap of {CURRENT_TRUTH_DEPTH_CAP}; the requested document was returned"
                    ),
                }),
            }
        } else {
            CurrentTruthResolution {
                head_path: current,
                chain,
                warning: None,
            }
        }
    }

    pub fn pending_intentions(&self, query: &str) -> Vec<PendingIntention> {
        let query_terms = normalized_terms(query);
        if query_terms.is_empty() {
            return Vec::new();
        }
        let mut matches = self
            .pending_intentions
            .iter()
            .filter_map(|intention| {
                let matched_terms = intention
                    .triggers
                    .iter()
                    .filter(|trigger| {
                        normalized_terms(trigger)
                            .iter()
                            .any(|term| query_terms.contains(term))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                (!matched_terms.is_empty()).then(|| PendingIntention {
                    path: intention.path.clone(),
                    title_line: intention.title_line.clone(),
                    due: intention.due.clone(),
                    status_note: intention_status_note(intention.due.as_deref()),
                    matched_terms,
                })
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            parsed_due(left.due.as_deref())
                .cmp(&parsed_due(right.due.as_deref()))
                .then_with(|| left.path.cmp(&right.path))
        });

        let mut retained = Vec::new();
        for intention in matches.into_iter().take(PENDING_INTENTION_LIMIT) {
            let mut candidate = retained.clone();
            candidate.push(intention.clone());
            let serialized_chars = serde_json::to_string(&candidate)
                .map(|value| value.chars().count())
                .unwrap_or(PENDING_INTENTION_CHAR_BUDGET + 1);
            if serialized_chars <= PENDING_INTENTION_CHAR_BUDGET {
                retained.push(intention);
            }
        }
        retained
    }
}

#[derive(Clone, Default)]
pub struct WorkspaceFeatureCache {
    inner: Arc<RwLock<HashMap<Uuid, Arc<WorkspaceFeatureSnapshot>>>>,
}

impl WorkspaceFeatureCache {
    pub async fn get(
        &self,
        user_id: Uuid,
        generation: i64,
    ) -> Option<Arc<WorkspaceFeatureSnapshot>> {
        self.inner
            .read()
            .await
            .get(&user_id)
            .filter(|snapshot| snapshot.generation == generation)
            .cloned()
    }

    pub async fn put(
        &self,
        user_id: Uuid,
        snapshot: WorkspaceFeatureSnapshot,
    ) -> Arc<WorkspaceFeatureSnapshot> {
        let snapshot = Arc::new(snapshot);
        self.inner.write().await.insert(user_id, snapshot.clone());
        snapshot
    }

    pub async fn invalidate(&self, user_id: Uuid) {
        self.inner.write().await.remove(&user_id);
    }
}

pub fn parse_frontmatter(content: &str) -> DerivedFrontmatter {
    let bounded = bounded_prefix(content, FRONTMATTER_LIMIT_BYTES);
    let mut lines = bounded.lines();
    if lines.next().map(str::trim) != Some("---") {
        return DerivedFrontmatter::default();
    }
    let mut fields = DerivedFrontmatter::default();
    let mut active_list: Option<&str> = None;
    let mut closed = false;
    for raw in lines {
        let line = raw.trim_end_matches('\r');
        if line.trim() == "---" {
            closed = true;
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(value) = line.trim().strip_prefix("- ") {
                push_list_value(&mut fields, active_list, value);
            }
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once(':') else {
            active_list = None;
            continue;
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        active_list = match key {
            "supersedes" | "trigger" if value.is_empty() => Some(key),
            _ => None,
        };
        match key {
            "supersedes" => {
                for item in inline_list(value) {
                    push_bounded(&mut fields.supersedes, item, 1_024, 32);
                }
            }
            "trigger" => {
                for item in inline_list(value) {
                    push_bounded(&mut fields.triggers, item, 80, 32);
                }
            }
            "kind" => fields.kind = bounded_scalar(value, 32).map(|v| v.to_lowercase()),
            "status" => fields.status = bounded_scalar(value, 32).map(|v| v.to_lowercase()),
            "due" => fields.due = bounded_scalar(value, 32),
            _ => {}
        }
    }
    if closed {
        fields
    } else {
        DerivedFrontmatter::default()
    }
}

pub fn supersession_warnings(
    fields: &DerivedFrontmatter,
    snapshot: &WorkspaceFeatureSnapshot,
) -> Vec<SupersessionWarning> {
    fields
        .supersedes
        .iter()
        .filter_map(|path| {
            if !safe_relative_path(path) {
                Some(SupersessionWarning {
                    kind: "invalid_supersedes_path".to_owned(),
                    path: path.clone(),
                    message: "supersedes paths must be safe vault-relative paths".to_owned(),
                })
            } else if !snapshot.contains_path(path) {
                Some(SupersessionWarning {
                    kind: "unresolved_supersedes_path".to_owned(),
                    path: path.clone(),
                    message: "the supersedes target does not resolve to a current workspace entry"
                        .to_owned(),
                })
            } else {
                None
            }
        })
        .collect()
}

fn chain_item(entry: &FeatureEntry) -> SupersessionChainItem {
    SupersessionChainItem {
        path: entry.path.clone(),
        entry_ref: format!("entry:{}", entry.entry_id),
    }
}

fn bounded_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn push_list_value(fields: &mut DerivedFrontmatter, active_list: Option<&str>, value: &str) {
    let Some(value) = bounded_scalar(value, 1_024) else {
        return;
    };
    match active_list {
        Some("supersedes") => push_bounded(&mut fields.supersedes, value, 1_024, 32),
        Some("trigger") => push_bounded(&mut fields.triggers, value, 80, 32),
        _ => {}
    }
}

fn inline_list(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let body = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed);
    body.split(',')
        .filter_map(|item| bounded_scalar(item, 1_024))
        .collect()
}

fn bounded_scalar(value: &str, max_chars: usize) -> Option<String> {
    let value = value.trim();
    let value = if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    };
    (!value.is_empty() && value.chars().count() <= max_chars).then(|| value.to_owned())
}

fn push_bounded(values: &mut Vec<String>, value: String, max_chars: usize, max_items: usize) {
    if values.len() < max_items
        && value.chars().count() <= max_chars
        && !values.iter().any(|existing| existing == &value)
    {
        values.push(value);
    }
}

fn title_line(content: &str, fallback: &str) -> String {
    let bounded = bounded_prefix(content, FRONTMATTER_LIMIT_BYTES);
    let mut in_frontmatter = bounded.lines().next().map(str::trim) == Some("---");
    let mut first = true;
    for raw in bounded.lines() {
        if first {
            first = false;
            if in_frontmatter {
                continue;
            }
        }
        let line = raw.trim();
        if in_frontmatter {
            if line == "---" {
                in_frontmatter = false;
            }
            continue;
        }
        if !line.is_empty() {
            return truncate_chars(line.trim_start_matches('#').trim(), 80);
        }
    }
    truncate_chars(fallback, 80)
}

fn normalized_terms(value: &str) -> HashSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter_map(|raw| {
            let lowered = raw.to_lowercase();
            (lowered.chars().count() >= 2).then(|| stem(&lowered))
        })
        .collect()
}

fn stem(value: &str) -> String {
    for suffix in ["ing", "ed", "es", "s"] {
        if value.chars().count() > suffix.chars().count() + 3
            && let Some(stem) = value.strip_suffix(suffix)
        {
            return stem.to_owned();
        }
    }
    value.to_owned()
}

fn parsed_due(value: Option<&str>) -> Option<NaiveDate> {
    value.and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
}

fn intention_status_note(due: Option<&str>) -> String {
    if parsed_due(due).is_some_and(|date| date < Utc::now().date_naive()) {
        "overdue".to_owned()
    } else {
        "pending".to_owned()
    }
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1_024
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        && !path.chars().any(char::is_control)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(path: &str, content: &str) -> WorkspaceFeatureDocument {
        WorkspaceFeatureDocument {
            entry_id: Uuid::new_v4(),
            path: path.to_owned(),
            title: path.trim_end_matches(".md").to_owned(),
            content: content.to_owned(),
        }
    }

    #[test]
    fn parser_accepts_only_closed_frontmatter_inside_four_kib() {
        let parsed = parse_frontmatter(
            "---\nsupersedes:\n  - Old.md\nkind: intention\ntrigger: [gmail, oauth-scopes]\ndue: 2026-08-03\nstatus: pending\n---\n# Follow up",
        );
        assert_eq!(parsed.supersedes, ["Old.md"]);
        assert_eq!(parsed.kind.as_deref(), Some("intention"));
        assert_eq!(parsed.triggers, ["gmail", "oauth-scopes"]);
        assert_eq!(parsed.due.as_deref(), Some("2026-08-03"));
        assert_eq!(parsed.status.as_deref(), Some("pending"));

        assert_eq!(
            parse_frontmatter("---\nkind: intention\n").kind,
            None,
            "an unterminated header must not become authority"
        );
        let oversized = format!("---\n{}\nkind: intention\n---", "x".repeat(4_096));
        assert_eq!(parse_frontmatter(&oversized).kind, None);
    }

    #[test]
    fn supersession_is_demotion_metadata_not_exclusion_and_walks_to_head() {
        let old = document("Old.md", "# Old");
        let old_id = old.entry_id;
        let middle = document("Middle.md", "---\nsupersedes: [Old.md]\n---\n# Middle");
        let head = document("Head.md", "---\nsupersedes:\n  - Middle.md\n---\n# Head");
        let head_id = head.entry_id;
        let snapshot = WorkspaceFeatureSnapshot::build(3, vec![old, middle, head]);
        assert_eq!(
            snapshot.superseded_by("Old.md"),
            Some(SupersessionAnnotation {
                path: "Middle.md".to_owned(),
                head_entry: format!("entry:{head_id}"),
            })
        );
        let resolution = snapshot.resolve_current_truth("Old.md");
        assert_eq!(resolution.head_path, "Head.md");
        assert_eq!(resolution.chain.len(), 3);
        assert_eq!(resolution.chain[0].entry_ref, format!("entry:{old_id}"));
        assert!(resolution.warning.is_none());
        assert!(snapshot.contains_path("Old.md"));
    }

    #[test]
    fn cycles_and_overlong_chains_fail_open_to_the_requested_document() {
        let a = document("A.md", "---\nsupersedes: [B.md]\n---");
        let b = document("B.md", "---\nsupersedes: [A.md]\n---");
        let snapshot = WorkspaceFeatureSnapshot::build(2, vec![a, b]);
        let resolution = snapshot.resolve_current_truth("A.md");
        assert_eq!(resolution.head_path, "A.md");
        assert_eq!(
            resolution
                .warning
                .as_ref()
                .map(|warning| warning.kind.as_str()),
            Some("supersession_cycle")
        );
        assert!(resolution.chain.len() >= 3);
    }

    #[test]
    fn pending_intentions_are_trigger_matched_pointer_only_and_bounded() {
        let pending = document(
            "Intentions/Gmail.md",
            "---\nkind: intention\ntrigger: [gmail, oauth-scopes]\ndue: 2099-08-03\nstatus: pending\n---\n# Review Gmail scopes\nNever inline this body.",
        );
        let done = document(
            "Intentions/Done.md",
            "---\nkind: intention\ntrigger: [gmail]\nstatus: done\n---\n# Done",
        );
        let snapshot = WorkspaceFeatureSnapshot::build(2, vec![pending, done]);
        let matches = snapshot.pending_intentions("Review the Gmail OAuth scope changes");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, "Intentions/Gmail.md");
        assert_eq!(matches[0].title_line, "Review Gmail scopes");
        assert_eq!(matches[0].matched_terms, ["gmail", "oauth-scopes"]);
        let encoded = serde_json::to_string(&matches).unwrap();
        assert!(encoded.chars().count() <= PENDING_INTENTION_CHAR_BUDGET);
        assert!(!encoded.contains("Never inline"));
    }
}

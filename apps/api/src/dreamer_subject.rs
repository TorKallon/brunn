//! Bounded, snapshot-local freshness for canonical subject research. Search
//! ranking is discovery, never proof that newer relevant evidence is absent.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth::AuthContext,
    error::{ApiError, ApiResult},
};

const MAX_NAMES: usize = 16;
const MAX_DEPENDENCIES: usize = 256;
const MAX_CHANGES: i64 = 2_000;
const MAX_CHANGE_BYTES: i64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubjectScope {
    pub subject_ref: String,
    pub subject_path: String,
    pub names: Vec<String>,
    pub checked_generation: i64,
    /// Full research dependencies, independent of a summary's claim citations.
    /// Missing on older immutable proposals; never hydrate those during reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<SubjectDependency>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubjectDependency {
    pub entry_ref: String,
    pub version: i64,
    pub path: String,
}

pub(crate) struct SubjectCheck {
    pub status: &'static str,
    pub reason: &'static str,
}

pub(crate) struct SubjectChanges {
    pub status: &'static str,
    pub reason: &'static str,
    pub relevant_ids: Vec<Uuid>,
    pub through_generation: i64,
    pub scanned_generation: i64,
}

fn result(status: &'static str, reason: &'static str) -> SubjectCheck {
    if status != "fresh" {
        tracing::info!(status, reason, "subject freshness rejected");
    }
    SubjectCheck { status, reason }
}

fn subject_id(reference: &str) -> Option<Uuid> {
    Uuid::parse_str(reference.strip_prefix("entry:")?).ok()
}

fn source_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    ![
        "derived/",
        "dreams/",
        ".brunn/",
        "agent-memory/",
        "location/",
        "evidence/location/",
        "memory/evidence/",
        "artifacts/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
        && path != "private/dreamer.md"
        && !crate::dreamer_review::sensitive_input_path(&path)
}

fn names(path: &str, metadata: &Value) -> ApiResult<Vec<String>> {
    let stem = path.rsplit('/').next().unwrap_or(path);
    let stem = stem.strip_suffix(".md").unwrap_or(stem);
    // entries.title is a display label inferred from the first Markdown heading,
    // which may describe a section rather than identify this canonical subject.
    let mut values = vec![stem.to_owned()];
    match &metadata["title"] {
        Value::Null => {}
        Value::String(title)
            if !title.trim().is_empty()
                && title.trim().len() <= 160
                && !title.contains(['\n', '\r']) =>
        {
            values.push(title.clone());
        }
        _ => {
            return Err(ApiError::invalid(
                "canonical subject names exceed the supported bounds",
            ));
        }
    }
    for key in ["aliases", "alias"] {
        match &metadata[key] {
            Value::String(value) => values.push(value.clone()),
            Value::Array(aliases) => {
                for alias in aliases {
                    let Some(alias) = alias.as_str() else {
                        return Err(ApiError::invalid(
                            "canonical subject aliases must be strings",
                        ));
                    };
                    values.push(alias.to_owned());
                }
            }
            Value::Null => {}
            _ => return Err(ApiError::invalid("canonical subject aliases are invalid")),
        }
    }
    let mut values = values
        .into_iter()
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    if values.is_empty()
        || values.len() > MAX_NAMES
        || values
            .iter()
            .any(|name| name.len() > 160 || name.contains(['\n', '\r']))
    {
        return Err(ApiError::invalid(
            "canonical subject names exceed the supported bounds",
        ));
    }
    Ok(values)
}

pub(crate) async fn create_scope(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    canonical_ref: &str,
    generation: i64,
) -> ApiResult<SubjectScope> {
    let id = subject_id(canonical_ref)
        .ok_or_else(|| ApiError::invalid("canonical subject must be an exact entry reference"))?;
    let row = sqlx::query("SELECT e.path,v.metadata,EXISTS(SELECT 1 FROM brunn.workspace_changes c WHERE c.user_id=e.user_id AND c.entry_id=e.id AND c.entry_version=e.current_version AND c.generation<=$3) AS admitted FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.id=$2 AND e.deleted_at IS NULL AND e.kind='markdown' AND v.content IS NOT NULL")
        .bind(auth.user_id.0).bind(id).bind(generation).fetch_optional(&mut **tx).await?
        .ok_or_else(|| ApiError::invalid("canonical subject is unavailable"))?;
    let path: String = row.get("path");
    let metadata: Value = row.get("metadata");
    if generation < 0
        || !row.get::<bool, _>("admitted")
        || !source_path(&path)
        || crate::dreamer_summary::protected_metadata(&metadata)
        || crate::dreamer_summary::generated_briefing_metadata(&metadata)
    {
        return Err(ApiError::invalid(
            "canonical subject is outside its evidence snapshot",
        ));
    }
    Ok(SubjectScope {
        subject_ref: canonical_ref.to_owned(),
        names: names(&path, &metadata)?,
        subject_path: path,
        checked_generation: generation,
        dependencies: Vec::new(),
    })
}

/// Attach server-resolved heads at candidate intake. Never call this on an
/// approved item: its exact serialized dependency manifest is immutable.
pub(crate) async fn bind_dependencies(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    scope: &mut SubjectScope,
    dependencies: &[(Uuid, i64)],
) -> ApiResult<()> {
    let mut next = scope.clone();
    next.dependencies.clear();
    let dependencies = dependency_set(&next, dependencies, true)
        .filter(|dependencies| valid_scope(&next, dependencies))
        .ok_or_else(|| {
            ApiError::invalid("subject dependencies are invalid or exceed their bound")
        })?;
    let ids = dependencies.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let rows = sqlx::query("SELECT e.id,e.path,e.current_version,v.metadata,EXISTS(SELECT 1 FROM brunn.workspace_changes c WHERE c.user_id=e.user_id AND c.entry_id=e.id AND c.entry_version=e.current_version AND c.generation<=$3) AS admitted FROM brunn.entries e JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version WHERE e.user_id=$1 AND e.id=ANY($2) AND e.deleted_at IS NULL AND e.kind='markdown' AND v.content IS NOT NULL")
        .bind(auth.user_id.0).bind(ids).bind(scope.checked_generation).fetch_all(&mut **tx).await?;
    for (id, version) in dependencies {
        let row = rows
            .iter()
            .find(|row| row.get::<Uuid, _>("id") == id)
            .ok_or_else(|| ApiError::invalid("subject dependency is unavailable"))?;
        let path: String = row.get("path");
        if row.get::<i64, _>("current_version") != version
            || !row.get::<bool, _>("admitted")
            || !source_path(&path)
            || path.len() > 1024
            || crate::dreamer_summary::protected_metadata(&row.get::<Value, _>("metadata"))
            || crate::dreamer_summary::generated_briefing_metadata(&row.get::<Value, _>("metadata"))
        {
            return Err(ApiError::invalid(
                "subject dependency changed or is outside its snapshot",
            ));
        }
        next.dependencies.push(SubjectDependency {
            entry_ref: format!("entry:{id}"),
            version,
            path,
        });
    }
    *scope = next;
    Ok(())
}

fn literal_pattern(values: impl IntoIterator<Item = String>) -> String {
    let mut values = values
        .into_iter()
        .map(|value| regex::escape(&value))
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    format!("(^|[^[:alnum:]_])({})([^[:alnum:]_]|$)", values.join("|"))
}

fn wiki_target_pattern(paths: impl IntoIterator<Item = String>) -> String {
    let mut targets = std::collections::BTreeSet::new();
    for path in paths {
        let filename = path.rsplit('/').next().unwrap_or(&path);
        targets.insert(regex::escape(filename));
        if let Some(stem) = filename.strip_suffix(".md") {
            targets.insert(regex::escape(stem));
        }
    }
    // A short link is a conservative relevance signal, never proof of an
    // identity merge. Match the complete target before its alias or fragment.
    format!(
        r"\[\[({})(\]\]|[|#])",
        targets.into_iter().collect::<Vec<_>>().join("|")
    )
}

fn valid_scope(scope: &SubjectScope, dependencies: &[(Uuid, i64)]) -> bool {
    scope.checked_generation >= 0
        && source_path(&scope.subject_path)
        && !scope.names.is_empty()
        && scope.names.len() <= MAX_NAMES
        && scope
            .names
            .iter()
            .all(|name| !name.is_empty() && name.len() <= 160 && !name.contains(['\n', '\r']))
        && dependencies.len() <= MAX_DEPENDENCIES
        && dependencies.iter().all(|(_, version)| *version > 0)
        && subject_id(&scope.subject_ref)
            .is_some_and(|canonical| dependencies.iter().any(|(id, _)| *id == canonical))
}

fn dependency_set(
    scope: &SubjectScope,
    dependencies: &[(Uuid, i64)],
    matching_versions: bool,
) -> Option<Vec<(Uuid, i64)>> {
    if scope.dependencies.len() > MAX_DEPENDENCIES || dependencies.len() > MAX_DEPENDENCIES {
        return None;
    }
    let mut merged = std::collections::BTreeMap::new();
    for (id, version) in dependencies
        .iter()
        .copied()
        .chain(scope.dependencies.iter().map(|source| {
            (
                subject_id(&source.entry_ref).unwrap_or(Uuid::nil()),
                source.version,
            )
        }))
    {
        if id.is_nil()
            || version < 1
            || merged
                .insert(id, version)
                .is_some_and(|old| matching_versions && old != version)
        {
            return None;
        }
    }
    if merged.len() > MAX_DEPENDENCIES
        || scope.dependencies.iter().any(|source| {
            source.path.is_empty() || source.path.len() > 1024 || !source_path(&source.path)
        })
    {
        return None;
    }
    Some(merged.into_iter().collect())
}

#[tracing::instrument(skip_all, fields(checked_generation = scope.checked_generation))]
pub(crate) async fn check_scope(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    scope: &SubjectScope,
    dependencies: &[(Uuid, i64)],
) -> ApiResult<SubjectCheck> {
    let Some(canonical) = subject_id(&scope.subject_ref) else {
        return Ok(result("unchecked", "invalid_subject_scope"));
    };
    let Some(dependencies) = dependency_set(scope, dependencies, true) else {
        return Ok(result("unchecked", "invalid_subject_dependencies"));
    };
    if !valid_scope(scope, &dependencies) {
        return Ok(result("unchecked", "invalid_subject_scope"));
    }
    let ids = dependencies.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let versions = dependencies
        .iter()
        .map(|(_, version)| *version)
        .collect::<Vec<_>>();
    let heads = sqlx::query("SELECT e.id,e.path,e.current_version,v.metadata FROM unnest($2::uuid[],$3::bigint[]) selected(id,version) JOIN brunn.entries e ON e.user_id=$1 AND e.id=selected.id JOIN brunn.entry_versions v ON v.user_id=e.user_id AND v.entry_id=e.id AND v.version=e.current_version JOIN LATERAL (SELECT original.metadata FROM brunn.entry_versions original WHERE original.user_id=e.user_id AND original.entry_id=e.id AND original.version=selected.version LIMIT 1) original ON true WHERE e.deleted_at IS NULL AND coalesce(original.metadata->>'kind','')<>'briefing_edition'")
        .bind(auth.user_id.0).bind(&ids).bind(versions).fetch_all(&mut **tx).await?;
    // Availability takes precedence over every freshness result, including for
    // historical summary reads. An earlier changed source cannot hide a later
    // revoked dependency and allow cached text to escape.
    if dependencies.iter().any(|(id, _)| {
        heads
            .iter()
            .find(|row| row.get::<Uuid, _>("id") == *id)
            .is_none_or(|head| {
                !source_path(head.get("path"))
                    || crate::dreamer_summary::protected_metadata(&head.get::<Value, _>("metadata"))
                    || crate::dreamer_summary::generated_briefing_metadata(
                        &head.get::<Value, _>("metadata"),
                    )
            })
    }) {
        return Ok(result("stale", "subject_source_unavailable"));
    }
    for (id, version) in &dependencies {
        let Some(head) = heads.iter().find(|row| row.get::<Uuid, _>("id") == *id) else {
            return Ok(result("stale", "subject_source_unavailable"));
        };
        if head.get::<i64, _>("current_version") != *version {
            // All dependencies passed the availability check above. Do not
            // log identities from an unavailable or protected manifest.
            tracing::info!(dependency_ref = %format!("entry:{id}"), expected_version = version,
                current_version = head.get::<i64, _>("current_version"), change = "version",
                "subject dependency changed");
            return Ok(result("stale", "subject_source_changed"));
        }
        if scope.dependencies.iter().any(|source| {
            subject_id(&source.entry_ref) == Some(*id)
                && source.path != head.get::<String, _>("path")
        }) {
            tracing::info!(dependency_ref = %format!("entry:{id}"), expected_version = version,
                current_version = head.get::<i64, _>("current_version"), change = "path",
                "subject dependency changed");
            return Ok(result("stale", "subject_source_changed"));
        }
        if !source_path(head.get("path"))
            || crate::dreamer_summary::protected_metadata(&head.get::<Value, _>("metadata"))
            || crate::dreamer_summary::generated_briefing_metadata(
                &head.get::<Value, _>("metadata"),
            )
        {
            return Ok(result("stale", "subject_source_unavailable"));
        }
        if *id == canonical {
            let current_names = names(head.get("path"), &head.get::<Value, _>("metadata"));
            if head.get::<String, _>("path") != scope.subject_path
                || current_names
                    .as_ref()
                    .map_or(true, |names| names != &scope.names)
            {
                return Ok(result("stale", "subject_identity_changed"));
            }
        }
    }
    let changes = research_changes(tx, auth, scope, &dependencies).await?;
    if !changes.relevant_ids.is_empty() {
        return Ok(result("stale", "subject_scope_changed"));
    }
    if changes.status != "complete" {
        return Ok(result("unchecked", changes.reason));
    }
    Ok(result("fresh", "subject_sources_and_scope_match"))
}

/// Return changes to reconcile without requiring old dependencies still to be
/// current. The caller may advance the coverage baseline only after complete
/// results have been admitted or explicitly dispositioned in the research job.
pub(crate) async fn research_changes(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    scope: &SubjectScope,
    dependencies: &[(Uuid, i64)],
) -> ApiResult<SubjectChanges> {
    research_change_page(tx, auth, scope, dependencies, i64::MAX).await
}

#[tracing::instrument(skip_all, fields(checked_generation = scope.checked_generation))]
pub(crate) async fn research_change_page(
    tx: &mut Transaction<'_, Postgres>,
    auth: &AuthContext,
    scope: &SubjectScope,
    dependencies: &[(Uuid, i64)],
    upper: i64,
) -> ApiResult<SubjectChanges> {
    let generation: i64 = sqlx::query_scalar(
        "SELECT coalesce(max(generation),0) FROM brunn.workspace_changes WHERE user_id=$1",
    )
    .bind(auth.user_id.0)
    .fetch_one(&mut **tx)
    .await?;
    let generation = generation.min(upper);
    let Some(dependencies) = dependency_set(scope, dependencies, false) else {
        return Ok(SubjectChanges {
            status: "unchecked",
            reason: "invalid_subject_dependencies",
            relevant_ids: vec![],
            through_generation: generation,
            scanned_generation: scope.checked_generation,
        });
    };
    if !valid_scope(scope, &dependencies) || scope.checked_generation > generation {
        return Ok(SubjectChanges {
            status: "unchecked",
            reason: "invalid_subject_scope",
            relevant_ids: vec![],
            through_generation: generation,
            scanned_generation: scope.checked_generation,
        });
    }
    let ids = dependencies.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let heads = sqlx::query("SELECT id,path FROM brunn.entries WHERE user_id=$1 AND id=ANY($2)")
        .bind(auth.user_id.0)
        .bind(&ids)
        .fetch_all(&mut **tx)
        .await?;
    let mut terms = scope.names.clone();
    terms.extend([scope.subject_ref.clone(), scope.subject_path.clone()]);
    terms.extend(scope.dependencies.iter().map(|source| source.path.clone()));
    terms.extend(ids.iter().map(|id| format!("entry:{id}")));
    for head in &heads {
        terms.push(head.get::<String, _>("path"));
    }
    let paths = std::iter::once(scope.subject_path.clone())
        .chain(scope.dependencies.iter().map(|source| source.path.clone()))
        .chain(heads.iter().map(|head| head.get::<String, _>("path")));
    let pattern = format!(
        "({})|({})",
        literal_pattern(terms),
        wiki_target_pattern(paths)
    );
    // Exclude generated churn before applying the page limit, except a rename
    // from an ordinary path. Materialize the page before expensive version
    // joins. Return only booleans: source bodies never enter foreground output.
    let rows = sqlx::query(r#"
        WITH page AS MATERIALIZED (
            SELECT c.*,previous.path AS previous_path,previous.entry_version AS previous_version
            FROM brunn.workspace_changes c
            LEFT JOIN LATERAL (
                SELECT old.path,old.entry_version FROM brunn.workspace_changes old
                WHERE old.user_id=c.user_id AND old.entry_id=c.entry_id AND old.generation<c.generation
                ORDER BY old.generation DESC LIMIT 1
            ) previous ON true
            LEFT JOIN brunn.entry_versions change_v ON change_v.user_id=c.user_id AND change_v.entry_id=c.entry_id AND change_v.version=c.entry_version
            LEFT JOIN brunn.entry_versions previous_v ON previous_v.user_id=c.user_id AND previous_v.entry_id=c.entry_id AND previous_v.version=previous.entry_version
            WHERE c.user_id=$1 AND c.generation>$2 AND c.generation<=$7
                AND ((lower(c.path) !~ '^(derived/|dreams/|\.brunn/|agent-memory/|location/|evidence/location/|memory/evidence/|artifacts/|private/dreamer\.md$)'
                    AND c.path !~* $8
                    AND NOT(coalesce(change_v.metadata,'{}'::jsonb) ?| ARRAY['dreamer_summary','dreamer_run','dreamer_review','dreamer_state','dreamer_receipt','dreamer_research'])
                    AND coalesce(change_v.metadata->>'kind','')<>'briefing_edition'
                    AND coalesce(change_v.metadata,'{}'::jsonb)::text NOT LIKE '%"evaluation_output": true%'
                    AND coalesce(change_v.metadata,'{}'::jsonb)::text NOT LIKE '%"exclude_from_same_day_evaluation_inputs": true%')
                    OR (previous.path IS NOT NULL AND lower(previous.path) !~ '^(derived/|dreams/|\.brunn/|agent-memory/|location/|evidence/location/|memory/evidence/|artifacts/|private/dreamer\.md$)'
                    AND previous.path !~* $8
                    AND NOT(coalesce(previous_v.metadata,'{}'::jsonb) ?| ARRAY['dreamer_summary','dreamer_run','dreamer_review','dreamer_state','dreamer_receipt','dreamer_research'])
                    AND coalesce(previous_v.metadata->>'kind','')<>'briefing_edition'
                    AND coalesce(previous_v.metadata,'{}'::jsonb)::text NOT LIKE '%"evaluation_output": true%'
                    AND coalesce(previous_v.metadata,'{}'::jsonb)::text NOT LIKE '%"exclude_from_same_day_evaluation_inputs": true%'))
            ORDER BY c.generation LIMIT $3
        ), sizes AS MATERIALIZED (
            SELECT page.*,e.id AS visible_entry,e.title,v.version AS visible_version,v.size_bytes,
                previous_v.version AS visible_previous,previous_v.size_bytes AS previous_size,
                (sum(CASE WHEN e.kind='markdown' THEN coalesce(v.size_bytes,0)+coalesce(previous_v.size_bytes,0) ELSE 0 END) OVER (ORDER BY page.generation))::bigint AS scanned_bytes
            FROM page LEFT JOIN brunn.entries e ON e.user_id=page.user_id AND e.id=page.entry_id
            LEFT JOIN brunn.entry_versions v ON v.user_id=page.user_id AND v.entry_id=page.entry_id AND v.version=page.entry_version
            LEFT JOIN brunn.entry_versions previous_v ON previous_v.user_id=page.user_id AND previous_v.entry_id=page.entry_id AND previous_v.version=page.previous_version
        )
        SELECT sizes.generation,sizes.entry_id,sizes.scanned_bytes,
            visible_entry IS NULL OR visible_version IS NULL OR previous_version IS NOT NULL AND visible_previous IS NULL AS version_unavailable,
            scanned_bytes>$5 AS byte_limit_exceeded,
            visible_entry IS NULL OR visible_version IS NULL OR previous_version IS NOT NULL AND visible_previous IS NULL
                OR scanned_bytes>$5 AS incomplete,
            entry_id=ANY($6) OR path ~* $4 OR coalesce(previous_path ~* $4,false) OR coalesce(title ~* $4,false)
                OR CASE WHEN scanned_bytes<=$5 THEN EXISTS (
                    SELECT 1 FROM brunn.entry_versions body
                    WHERE body.user_id=sizes.user_id AND body.entry_id=sizes.entry_id
                        AND body.version IN (sizes.entry_version,sizes.previous_version)
                        AND (body.content ~* $4 OR body.metadata->>'title' ~* $4)
                ) ELSE false END AS relevant
        FROM sizes ORDER BY generation
    "#).bind(auth.user_id.0).bind(scope.checked_generation).bind(MAX_CHANGES+1)
        .bind(pattern).bind(MAX_CHANGE_BYTES).bind(ids).bind(generation).bind(crate::dreamer_review::SENSITIVE_INPUT_PATH).fetch_all(&mut **tx).await?;
    let relevant_ids = rows
        .iter()
        .take(MAX_CHANGES as usize)
        .filter(|row| row.get::<bool, _>("relevant"))
        .map(|row| row.get::<Uuid, _>("entry_id"))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let (status, reason) = if rows
        .iter()
        .take(MAX_CHANGES as usize)
        .any(|row| row.get::<bool, _>("incomplete"))
    {
        ("unchecked", "subject_change_coverage_incomplete")
    } else if rows.len() > MAX_CHANGES as usize {
        ("unchecked", "subject_change_check_limit")
    } else {
        ("complete", "subject_changes_complete")
    };
    let scanned_generation = if status == "complete" {
        generation
    } else {
        rows.iter()
            .take(MAX_CHANGES as usize)
            .take_while(|row| !row.get::<bool, _>("incomplete"))
            .last()
            .map_or(scope.checked_generation, |row| row.get("generation"))
    };
    let first_relevant = rows
        .iter()
        .take(MAX_CHANGES as usize)
        .find(|row| row.get::<bool, _>("relevant"));
    let first_incomplete = rows
        .iter()
        .take(MAX_CHANGES as usize)
        .find(|row| row.get::<bool, _>("incomplete"));
    if first_relevant.is_some() || status != "complete" {
        // Generations identify the exact ordinary change for authorized header
        // lookup. Never log source text, paths or unavailable entry identities.
        tracing::info!(
            status,
            reason,
            through_generation = generation,
            scanned_generation,
            first_relevant_generation = first_relevant.map(|row| row.get::<i64, _>("generation")),
            first_incomplete_generation =
                first_incomplete.map(|row| row.get::<i64, _>("generation")),
            version_unavailable =
                first_incomplete.map(|row| row.get::<bool, _>("version_unavailable")),
            byte_limit_exceeded =
                first_incomplete.map(|row| row.get::<bool, _>("byte_limit_exceeded")),
            scanned_bytes_at_incomplete =
                first_incomplete.map(|row| row.get::<i64, _>("scanned_bytes")),
            change_count = rows.len(),
            row_limit = MAX_CHANGES,
            byte_limit = MAX_CHANGE_BYTES,
            "subject change coverage checked"
        );
    }
    Ok(SubjectChanges {
        status,
        reason,
        relevant_ids,
        through_generation: generation,
        scanned_generation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_scope_bytes_do_not_gain_an_empty_dependency_manifest() {
        let old = serde_json::json!({"subject_ref":format!("entry:{}",Uuid::now_v7()),"subject_path":"People/Aster.md","names":["aster"],"checked_generation":42});
        let scope: SubjectScope = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(scope).unwrap(), old);
    }

    #[test]
    fn names_are_source_owned_and_matching_is_literal() {
        let source_names = names(
            "People/A+B.md",
            &serde_json::json!({"aliases":["A. B.", "A+B"]}),
        )
        .unwrap();
        let pattern = regex::RegexBuilder::new(&literal_pattern(source_names))
            .case_insensitive(true)
            .build()
            .unwrap();
        assert!(pattern.is_match("Discussed A+B's new outcome"));
        assert!(pattern.is_match("[[A. B.]]"));
        assert!(!pattern.is_match("Other AAAB project"));
        assert!(!pattern.is_match("XA+B-other"));
        assert!(
            names(
                "People/Owner.md",
                &serde_json::json!({"aliases":[{"name":"model-selected"}]})
            )
            .is_err()
        );
        let links = regex::Regex::new(&wiki_target_pattern(["Imported/A+B outcome.md".to_owned()]))
            .unwrap();
        assert!(links.is_match("Follow [[A+B outcome]]"));
        assert!(links.is_match("[[A+B outcome#Completed|the outcome]]"));
        assert!(!links.is_match("[[A+B outcomes]]"));
        assert!(!links.is_match("A+B outcome is unrelated plain text"));
    }

    #[test]
    fn subject_names_use_filename_and_explicit_metadata_without_heading_blacklists() {
        assert_eq!(
            names("Projects/Ithrion.md", &serde_json::json!({})).unwrap(),
            ["ithrion"]
        );
        assert_eq!(
            names("Projects/Ithrion.md", &serde_json::json!({"title":"Aurora Atlas", "aliases":["North Relay"], "alias":"Purpose"})).unwrap(),
            ["aurora atlas", "ithrion", "north relay", "purpose"]
        );
        for metadata in [
            serde_json::json!({"title":"Purpose"}),
            serde_json::json!({"aliases":["Purpose"]}),
            serde_json::json!({"alias":"Purpose"}),
        ] {
            assert_eq!(
                names("Projects/Ithrion.md", &metadata).unwrap(),
                ["ithrion", "purpose"]
            );
        }
        assert_eq!(
            names("Projects/Purpose.md", &serde_json::json!({"title":null})).unwrap(),
            ["purpose"]
        );
    }

    #[test]
    fn explicit_subject_titles_must_be_bounded_nonempty_single_line_strings() {
        for title in [
            serde_json::json!(""),
            serde_json::json!("   "),
            serde_json::json!("\nPurpose"),
            serde_json::json!("Purpose\r"),
            serde_json::json!("x".repeat(161)),
            serde_json::json!(42),
            serde_json::json!(false),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            let error =
                names("Projects/Ithrion.md", &serde_json::json!({"title":title})).unwrap_err();
            assert!(
                matches!(error, ApiError::Public { message, .. } if message == "canonical subject names exceed the supported bounds")
            );
        }
        assert_eq!(
            names(
                "Projects/Ithrion.md",
                &serde_json::json!({"title":"  Aurora Atlas  "})
            )
            .unwrap(),
            ["aurora atlas", "ithrion"]
        );
    }
}

/// Stable invocation text shared by the simple workspace request path and the
/// D09 SQL-drift contract. The function bodies remain migration-owned because
/// PostgreSQL installs them there; `performance_eval.py` fingerprints those
/// bodies and the installed `pg_proc.prosrc` before asserting their plans.
pub const SIMPLE_LEXICAL_CANDIDATES_SQL: &str =
    "SELECT * FROM straylight.workspace_lexical_candidates_v2($1,$2)";

pub const SIMPLE_LEXICAL_CANDIDATES_WITH_GENERATION_SQL: &str = r#"
WITH generation AS (
  SELECT coalesce(max(change.generation),0) AS workspace_generation
  FROM straylight.workspace_changes AS change
  WHERE change.user_id=$1
)
SELECT generation.workspace_generation,candidate.*
FROM generation
LEFT JOIN LATERAL straylight.workspace_lexical_candidates_v2($2,$3) AS candidate
  ON true
"#;

pub const SIMPLE_SEMANTIC_CANDIDATES_SQL: &str =
    "SELECT * FROM straylight.workspace_semantic_candidates_v2($1,$2)";

pub const SIMPLE_ENTRY_LINK_CANDIDATES_SQL: &str = r#"
WITH candidates AS MATERIALIZED (
  SELECT entry.id
  FROM straylight.entries AS entry
  WHERE entry.user_id=$1
    AND entry.deleted_at IS NULL
    AND lower(normalize(regexp_replace(entry.path,'^.*/',''), NFC))=ANY($2)
  LIMIT 2
)
SELECT entry.id,entry.path,entry.title,entry.kind,entry.media_type,
       entry.current_version,entry.updated_at,
       version.id AS version_id,version.content_sha256,version.content,
       version.object_key,version.object_version_id,version.size_bytes,
       version.metadata,
       (SELECT coalesce(max(change.generation),0)
        FROM straylight.workspace_changes AS change
        WHERE change.user_id=entry.user_id) AS workspace_generation
FROM candidates AS candidate
JOIN straylight.entries AS entry ON entry.id=candidate.id
JOIN straylight.entry_versions AS version
  ON version.user_id=entry.user_id
 AND version.entry_id=entry.id
 AND version.version=entry.current_version
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_invocations_remain_single_statement_and_parameterized() {
        for (sql, function) in [
            (
                SIMPLE_LEXICAL_CANDIDATES_SQL,
                "straylight.workspace_lexical_candidates_v2",
            ),
            (
                SIMPLE_SEMANTIC_CANDIDATES_SQL,
                "straylight.workspace_semantic_candidates_v2",
            ),
        ] {
            assert!(sql.starts_with("SELECT * FROM "));
            assert!(sql.contains(function));
            assert!(sql.ends_with("($1,$2)"));
            assert!(!sql.contains(';'));
        }

        assert!(
            SIMPLE_LEXICAL_CANDIDATES_WITH_GENERATION_SQL
                .contains("LEFT JOIN LATERAL straylight.workspace_lexical_candidates_v2($2,$3)")
        );
        assert!(SIMPLE_LEXICAL_CANDIDATES_WITH_GENERATION_SQL.contains("WHERE change.user_id=$1"));
        assert!(!SIMPLE_LEXICAL_CANDIDATES_WITH_GENERATION_SQL.contains(';'));
        assert!(SIMPLE_ENTRY_LINK_CANDIDATES_SQL.contains("entry.user_id=$1"));
        assert!(SIMPLE_ENTRY_LINK_CANDIDATES_SQL.contains("regexp_replace(entry.path"));
        assert!(SIMPLE_ENTRY_LINK_CANDIDATES_SQL.contains("=ANY($2)"));
        assert!(!SIMPLE_ENTRY_LINK_CANDIDATES_SQL.contains("entry.title, NFC"));
        assert!(SIMPLE_ENTRY_LINK_CANDIDATES_SQL.contains("LIMIT 2"));
        assert!(SIMPLE_ENTRY_LINK_CANDIDATES_SQL.contains("version.version=entry.current_version"));
        assert!(!SIMPLE_ENTRY_LINK_CANDIDATES_SQL.contains(';'));
    }
}

/// Stable invocation text shared by the simple workspace request path and the
/// D09 SQL-drift contract. The function bodies remain migration-owned because
/// PostgreSQL installs them there; `performance_eval.py` fingerprints those
/// bodies and the installed `pg_proc.prosrc` before asserting their plans.
pub const SIMPLE_LEXICAL_CANDIDATES_SQL: &str =
    "SELECT * FROM straylight.workspace_lexical_candidates($1)";

pub const SIMPLE_LEXICAL_CANDIDATES_WITH_GENERATION_SQL: &str = r#"
WITH generation AS (
  SELECT coalesce(max(change.generation),0) AS workspace_generation
  FROM straylight.workspace_changes AS change
  WHERE change.user_id=$1
)
SELECT generation.workspace_generation,candidate.*
FROM generation
LEFT JOIN LATERAL straylight.workspace_lexical_candidates($2) AS candidate
  ON true
"#;

pub const SIMPLE_SEMANTIC_CANDIDATES_SQL: &str =
    "SELECT * FROM straylight.workspace_semantic_candidates($1)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_invocations_remain_single_statement_and_parameterized() {
        for (sql, function) in [
            (
                SIMPLE_LEXICAL_CANDIDATES_SQL,
                "straylight.workspace_lexical_candidates",
            ),
            (
                SIMPLE_SEMANTIC_CANDIDATES_SQL,
                "straylight.workspace_semantic_candidates",
            ),
        ] {
            assert!(sql.starts_with("SELECT * FROM "));
            assert!(sql.contains(function));
            assert!(sql.ends_with("($1)"));
            assert!(!sql.contains(';'));
        }

        assert!(
            SIMPLE_LEXICAL_CANDIDATES_WITH_GENERATION_SQL
                .contains("LEFT JOIN LATERAL straylight.workspace_lexical_candidates($2)")
        );
        assert!(SIMPLE_LEXICAL_CANDIDATES_WITH_GENERATION_SQL.contains("WHERE change.user_id=$1"));
        assert!(!SIMPLE_LEXICAL_CANDIDATES_WITH_GENERATION_SQL.contains(';'));
    }
}

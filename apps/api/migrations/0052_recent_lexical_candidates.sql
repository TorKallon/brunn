CREATE OR REPLACE FUNCTION straylight.workspace_lexical_candidates(
  p_query text
)
RETURNS TABLE (
  entry_id uuid,
  path text,
  heading text,
  content text,
  score double precision,
  title text,
  current_version bigint,
  content_sha256 straylight.sha256_hex
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
  WITH context AS (
    SELECT straylight_auth.setting_uuid('app.current_user_id') AS user_id
    WHERE straylight_auth.context_is_valid()
  ), requested AS (
    SELECT websearch_to_tsquery('english', p_query) AS query
  ), recent_entry_ids AS MATERIALIZED (
    SELECT DISTINCT recent.entry_id
    FROM (
      SELECT change.entry_id
      FROM straylight.workspace_changes AS change
      CROSS JOIN context
      WHERE change.user_id=context.user_id
      ORDER BY change.generation DESC
      LIMIT 256
    ) AS recent
  ), recent_matches AS MATERIALIZED (
    SELECT chunk.id,chunk.entry_id,
           (
             ts_rank_cd(chunk.search_vector,requested.query,32)
             + 0.000001
           )::double precision AS score
    FROM recent_entry_ids AS recent
    CROSS JOIN context
    CROSS JOIN requested
    JOIN straylight.search_chunks AS chunk
      ON chunk.user_id=context.user_id
     AND chunk.entry_id=recent.entry_id
    WHERE chunk.search_vector @@ requested.query
      AND chunk.path NOT LIKE '.straylight/checkpoints/%'
  ), recent_density AS MATERIALIZED (
    SELECT count(DISTINCT entry_id) AS matching_entries
    FROM recent_matches
  ), index_matches AS MATERIALIZED (
    SELECT chunk.id,chunk.entry_id,
           ts_rank_cd(chunk.search_vector,requested.query,32)::double precision AS score
    FROM straylight.search_chunks AS chunk
    CROSS JOIN context
    CROSS JOIN requested
    CROSS JOIN recent_density
    WHERE recent_density.matching_entries < 128
      AND chunk.user_id=context.user_id
      AND chunk.search_vector @@ requested.query
      AND chunk.path NOT LIKE '.straylight/checkpoints/%'
    LIMIT 4096
  ), bounded_matches AS MATERIALIZED (
    SELECT recent.id,recent.entry_id,recent.score
    FROM recent_matches AS recent
    UNION ALL
    SELECT matched.id,matched.entry_id,matched.score
    FROM index_matches AS matched
    WHERE NOT EXISTS (
      SELECT 1
      FROM recent_matches AS recent
      WHERE recent.id=matched.id
    )
  ), ranked_chunks AS MATERIALIZED (
    SELECT matched.*,
           row_number() OVER (
             PARTITION BY matched.entry_id
             ORDER BY matched.score DESC,matched.id
           ) AS section_rank
    FROM bounded_matches AS matched
  ), ranked_entries AS MATERIALIZED (
    SELECT matched.entry_id,max(matched.score) AS entry_score
    FROM ranked_chunks AS matched
    GROUP BY matched.entry_id
    ORDER BY entry_score DESC,matched.entry_id
    LIMIT 48
  ), ranked AS MATERIALIZED (
    SELECT chunk.id,chunk.entry_id,chunk.score,
           entry.entry_score,chunk.section_rank
    FROM ranked_chunks AS chunk
    JOIN ranked_entries AS entry USING (entry_id)
    WHERE chunk.section_rank <= 3
  )
  SELECT ranked.entry_id,chunk.path,chunk.heading,chunk.content,
         ranked.score,entry.title,entry.current_version,
         version.content_sha256
  FROM ranked
  JOIN context ON true
  JOIN straylight.search_chunks AS chunk
    ON chunk.user_id=context.user_id AND chunk.id=ranked.id
  JOIN straylight.entries AS entry
    ON entry.user_id=context.user_id AND entry.id=ranked.entry_id
  JOIN straylight.entry_versions AS version
    ON version.user_id=entry.user_id
   AND version.entry_id=entry.id
   AND version.version=entry.current_version
  WHERE entry.deleted_at IS NULL
  ORDER BY ranked.entry_score DESC,ranked.entry_id,
           ranked.section_rank,ranked.score DESC,ranked.id;
$$;

REVOKE ALL ON FUNCTION straylight.workspace_lexical_candidates(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight.workspace_lexical_candidates(text)
  TO app_ro,app_rw;

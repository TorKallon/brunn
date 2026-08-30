-- Lexical candidate selection ranked an unordered LIMIT 4096 sample of every
-- matching chunk, detoasting each tsvector for ts_rank_cd. On the production
-- corpus a common term matched ~6,800 chunks: ~160ms warm and multiple
-- seconds cold, which tripped the 2s lexical lane budget and degraded the
-- first dreamer run to exact-read-only evidence. Collect candidate ids with
-- the original early-terminating LIMIT 4096 (never touching search_vector),
-- keep the newest 1024 by uuidv7 order, and fetch and rank only those rows.
-- Worst-case moderate terms drop from ~160ms to ~35ms and broad terms keep
-- their early-termination bound. Everything else is unchanged from
-- migration 0067.

CREATE OR REPLACE FUNCTION straylight.workspace_lexical_candidates_v2(
  p_query text,
  p_sort text
)
RETURNS TABLE (
  entry_id uuid,
  path text,
  heading text,
  content text,
  score double precision,
  title text,
  current_version bigint,
  content_sha256 straylight.sha256_hex,
  updated_at timestamptz
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
    SELECT websearch_to_tsquery('english', p_query) AS query,
           CASE
             WHEN p_sort IN ('best_match','last_modified','title') THEN p_sort
             ELSE NULL
           END AS sort
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
  ), index_match_pool AS MATERIALIZED (
    -- Id-only candidate collection preserving the early-termination LIMIT:
    -- an ordered scan here would visit every matching heap tuple, which a
    -- broad term makes O(corpus). The pool inherits the original arbitrary
    -- 4096 sample semantics at a fraction of the cost because it never
    -- touches search_vector.
    SELECT chunk.id,chunk.entry_id
    FROM straylight.search_chunks AS chunk
    CROSS JOIN context
    CROSS JOIN requested
    CROSS JOIN recent_density
    WHERE (
        requested.sort <> 'best_match'
        OR recent_density.matching_entries < 128
      )
      AND chunk.user_id=context.user_id
      AND chunk.search_vector @@ requested.query
      AND chunk.path NOT LIKE '.straylight/checkpoints/%'
    LIMIT 4096
  ), index_match_ids AS MATERIALIZED (
    -- Rank only the newest quarter of the pool: detoasting tsvectors for
    -- ts_rank_cd was the dominant lexical cost.
    SELECT pool.id,pool.entry_id
    FROM index_match_pool AS pool
    ORDER BY pool.id DESC
    LIMIT 1024
  ), index_matches AS MATERIALIZED (
    SELECT chunk.id,chunk.entry_id,
           ts_rank_cd(chunk.search_vector,requested.query,32)::double precision AS score
    FROM index_match_ids AS candidate
    CROSS JOIN requested
    JOIN straylight.search_chunks AS chunk
      ON chunk.id=candidate.id
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
  ), entry_scores AS MATERIALIZED (
    SELECT matched.entry_id,max(matched.score) AS entry_score
    FROM ranked_chunks AS matched
    GROUP BY matched.entry_id
  ), eligible_entries AS MATERIALIZED (
    SELECT scored.entry_id,scored.entry_score,entry.path,entry.title,
           entry.current_version,entry.updated_at,version.content_sha256
    FROM entry_scores AS scored
    CROSS JOIN context
    CROSS JOIN requested
    JOIN straylight.entries AS entry
      ON entry.user_id=context.user_id AND entry.id=scored.entry_id
    JOIN straylight.entry_versions AS version
      ON version.user_id=entry.user_id
     AND version.entry_id=entry.id
     AND version.version=entry.current_version
    WHERE requested.sort IS NOT NULL
      AND entry.deleted_at IS NULL
  ), ranked_entries AS MATERIALIZED (
    SELECT *
    FROM eligible_entries
    ORDER BY
      CASE WHEN p_sort='best_match' THEN entry_score END DESC NULLS LAST,
      CASE WHEN p_sort='last_modified' THEN updated_at END DESC NULLS LAST,
      CASE WHEN p_sort='last_modified' THEN entry_score END DESC NULLS LAST,
      CASE WHEN p_sort='title' THEN lower(title) END ASC NULLS LAST,
      CASE WHEN p_sort='title' THEN updated_at END DESC NULLS LAST,
      updated_at DESC,path,entry_id
    LIMIT 64
  ), ranked AS MATERIALIZED (
    SELECT chunk.id,chunk.entry_id,chunk.score,chunk.section_rank,
           entry.entry_score,entry.path,entry.title,entry.current_version,
           entry.content_sha256,entry.updated_at
    FROM ranked_chunks AS chunk
    JOIN ranked_entries AS entry USING (entry_id)
    WHERE chunk.section_rank <= 3
  )
  SELECT ranked.entry_id,chunk.path,chunk.heading,chunk.content,
         ranked.score,ranked.title,ranked.current_version,
         ranked.content_sha256,ranked.updated_at
  FROM ranked
  JOIN context ON true
  JOIN straylight.search_chunks AS chunk
    ON chunk.user_id=context.user_id AND chunk.id=ranked.id
  ORDER BY
    CASE WHEN p_sort='best_match' THEN ranked.entry_score END DESC NULLS LAST,
    CASE WHEN p_sort='last_modified' THEN ranked.updated_at END DESC NULLS LAST,
    CASE WHEN p_sort='last_modified' THEN ranked.entry_score END DESC NULLS LAST,
    CASE WHEN p_sort='title' THEN lower(ranked.title) END ASC NULLS LAST,
    CASE WHEN p_sort='title' THEN ranked.updated_at END DESC NULLS LAST,
    ranked.updated_at DESC,ranked.path,ranked.entry_id,
    ranked.section_rank,ranked.score DESC,ranked.id;
$$;

REVOKE ALL ON FUNCTION straylight.workspace_lexical_candidates_v2(text,text)
  FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight.workspace_lexical_candidates_v2(text,text)
  TO app_ro,app_rw;

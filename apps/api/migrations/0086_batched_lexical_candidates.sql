-- The lexical fallback lane evaluates at most four selective AND queries. Keep
-- those alternatives as independent indexed branches so PostgreSQL can stop
-- each GIN scan early, then deduplicate and hydrate the bounded union once.
-- pg_prewarm is loaded by the production database image and preserves the hot
-- relation set across ordinary restarts; creating the extension activates its
-- database-local state without changing either candidate function's volatility.

CREATE EXTENSION IF NOT EXISTS pg_prewarm;

-- Migration 0013 grants application roles USAGE on public so they can resolve
-- required extension types. PostgreSQL extensions grant function execution to
-- PUBLIC by default; pg_prewarm's cache and worker controls are not part of the
-- application contract and remain database-owner/admin only.
REVOKE EXECUTE ON FUNCTION public.pg_prewarm(regclass,text,text,bigint,bigint)
  FROM PUBLIC,app_ro,app_rw;
REVOKE EXECUTE ON FUNCTION public.autoprewarm_start_worker()
  FROM PUBLIC,app_ro,app_rw;
REVOKE EXECUTE ON FUNCTION public.autoprewarm_dump_now()
  FROM PUBLIC,app_ro,app_rw;

CREATE OR REPLACE FUNCTION brunn.workspace_lexical_candidates_v3(
  p_queries text[],
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
  content_sha256 brunn.sha256_hex,
  updated_at timestamptz
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
  WITH context AS (
    SELECT brunn_auth.setting_uuid('app.current_user_id') AS user_id
    WHERE brunn_auth.context_is_valid()
  ), parameters AS (
    SELECT CASE
             WHEN p_sort IN ('best_match','last_modified','title') THEN p_sort
             ELSE NULL
           END AS sort
  ), requested AS MATERIALIZED (
    SELECT raw.query_ordinal,
           websearch_to_tsquery('english',raw.query_text) AS query
    FROM unnest(p_queries) WITH ORDINALITY
      AS raw(query_text,query_ordinal)
    WHERE raw.query_ordinal <= 4
      AND nullif(btrim(raw.query_text),'') IS NOT NULL
  ), recent_entry_ids AS MATERIALIZED (
    SELECT DISTINCT recent.entry_id
    FROM (
      SELECT change.entry_id
      FROM brunn.workspace_changes AS change
      CROSS JOIN context
      WHERE change.user_id=context.user_id
      ORDER BY change.generation DESC
      LIMIT 256
    ) AS recent
  ), recent_matches AS MATERIALIZED (
    SELECT requested.query_ordinal,chunk.id,chunk.entry_id,
           (
             ts_rank_cd(chunk.search_vector,requested.query,32)
             + 0.000001
           )::double precision AS score
    FROM requested
    CROSS JOIN context
    JOIN brunn.search_chunks AS chunk
      ON chunk.user_id=context.user_id
    JOIN recent_entry_ids AS recent
      ON recent.entry_id=chunk.entry_id
    WHERE chunk.search_vector @@ requested.query
      AND chunk.path NOT LIKE '.brunn/checkpoints/%'
  ), recent_density AS MATERIALIZED (
    SELECT requested.query_ordinal,
           count(DISTINCT recent.entry_id) AS matching_entries
    FROM requested
    LEFT JOIN recent_matches AS recent USING (query_ordinal)
    GROUP BY requested.query_ordinal
  ), index_match_pool AS MATERIALIZED (
    SELECT requested.query_ordinal,pool.id,pool.entry_id
    FROM requested
    JOIN recent_density USING (query_ordinal)
    CROSS JOIN context
    CROSS JOIN parameters
    CROSS JOIN LATERAL (
      -- This LATERAL branch is planned and bounded independently for every
      -- query. Do not combine the tsqueries with OR: that restores the broad
      -- heap scan that this function exists to avoid.
      SELECT chunk.id,chunk.entry_id
      FROM brunn.search_chunks AS chunk
      WHERE (
          parameters.sort <> 'best_match'
          OR recent_density.matching_entries < 128
        )
        AND chunk.user_id=context.user_id
        AND chunk.search_vector @@ requested.query
        AND chunk.path NOT LIKE '.brunn/checkpoints/%'
      LIMIT 1024
    ) AS pool
  ), index_match_ids AS MATERIALIZED (
    -- Rank only one quarter of each independently bounded id pool. Relevance
    -- uses newest ids as its deterministic sample; non-relevance sorts select
    -- their candidate entries by the requested metadata before ts_rank_cd ever
    -- detoasts a search vector.
    SELECT candidate.query_ordinal,candidate.id,candidate.entry_id
    FROM (
      SELECT pool.*,
             row_number() OVER (
               PARTITION BY pool.query_ordinal
               ORDER BY
                 CASE WHEN p_sort='best_match' THEN pool.id END DESC NULLS LAST,
                 CASE WHEN p_sort='last_modified' THEN entry.updated_at END DESC NULLS LAST,
                 CASE WHEN p_sort='title' THEN lower(entry.title) END ASC NULLS LAST,
                 CASE WHEN p_sort='title' THEN entry.updated_at END DESC NULLS LAST,
                 pool.id DESC
             ) AS pool_rank
      FROM index_match_pool AS pool
      JOIN context ON true
      JOIN brunn.entries AS entry
        ON entry.user_id=context.user_id
       AND entry.id=pool.entry_id
       AND entry.deleted_at IS NULL
    ) AS candidate
    WHERE candidate.pool_rank <= 256
  ), index_matches AS MATERIALIZED (
    SELECT candidate.query_ordinal,chunk.id,chunk.entry_id,
           ts_rank_cd(chunk.search_vector,requested.query,32)::double precision AS score
    FROM index_match_ids AS candidate
    JOIN requested USING (query_ordinal)
    CROSS JOIN context
    JOIN brunn.search_chunks AS chunk
      ON chunk.user_id=context.user_id
     AND chunk.id=candidate.id
  ), bounded_matches AS MATERIALIZED (
    SELECT recent.query_ordinal,recent.id,recent.entry_id,recent.score
    FROM recent_matches AS recent
    UNION ALL
    SELECT matched.query_ordinal,matched.id,matched.entry_id,matched.score
    FROM index_matches AS matched
    WHERE NOT EXISTS (
      SELECT 1
      FROM recent_matches AS recent
      WHERE recent.query_ordinal=matched.query_ordinal
        AND recent.id=matched.id
    )
  ), deduplicated_matches AS MATERIALIZED (
    SELECT matched.id,matched.entry_id,max(matched.score) AS score
    FROM bounded_matches AS matched
    GROUP BY matched.id,matched.entry_id
  ), ranked_chunks AS MATERIALIZED (
    SELECT matched.*,
           row_number() OVER (
             PARTITION BY matched.entry_id
             ORDER BY matched.score DESC,matched.id
           ) AS section_rank
    FROM deduplicated_matches AS matched
  ), entry_scores AS MATERIALIZED (
    SELECT matched.entry_id,max(matched.score) AS entry_score
    FROM ranked_chunks AS matched
    GROUP BY matched.entry_id
  ), eligible_entries AS MATERIALIZED (
    SELECT scored.entry_id,scored.entry_score,entry.path,entry.title,
           entry.current_version,entry.updated_at,version.content_sha256
    FROM entry_scores AS scored
    CROSS JOIN context
    CROSS JOIN parameters
    JOIN brunn.entries AS entry
      ON entry.user_id=context.user_id AND entry.id=scored.entry_id
    JOIN brunn.entry_versions AS version
      ON version.user_id=entry.user_id
     AND version.entry_id=entry.id
     AND version.version=entry.current_version
    WHERE parameters.sort IS NOT NULL
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
  JOIN brunn.search_chunks AS chunk
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

REVOKE ALL ON FUNCTION brunn.workspace_lexical_candidates_v3(text[],text)
  FROM PUBLIC;
GRANT EXECUTE ON FUNCTION brunn.workspace_lexical_candidates_v3(text[],text)
  TO app_ro,app_rw;

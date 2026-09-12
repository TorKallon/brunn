-- Inline the validated user context in the semantic candidate function.
--
-- `context` is referenced twice in the 0067 body, so PostgreSQL 17 materializes
-- the CTE and joins it as an opaque one-row relation. The planner then cannot
-- see the current user id as a constant, estimates the per-user chunk count
-- from `user_id` statistics alone, and on a multi-user corpus prefers a top-N
-- sort over every one of that user's vectors (via
-- `search_chunks_semantic_coverage_idx`) to the HNSW walk, plus a bitmap scan
-- of all of that user's entries instead of a keyed join. Measured on Nyx for a
-- 128,142-vector user among 1,335 users: 565,000 shared buffers and 4.6-15.6 s
-- per call, versus 4,751 buffers and under 10 ms with the context inlined, with
-- identical results. Production has one embedded user today, so its plan is
-- unaffected; this removes the regression before more users sign up.
--
-- The lexical functions (0081, 0098) deliberately keep their materialized
-- context: with the user id visible, the planner switched `recent_entry_ids`
-- from the (user_id, entry_id, generation) index-only scan to a backward
-- primary-key scan across every user's changes (740 -> 21,643 buffers on the
-- same corpus). Their measured cost is already bounded (under 1.2K buffers per
-- call in production).
--
-- The body is otherwise byte-for-byte the 0067 definition; CREATE OR REPLACE
-- preserves that migration's grants.

CREATE OR REPLACE FUNCTION brunn.workspace_semantic_candidates_v2(
  p_embedding vector(1536),
  p_sort text
)
RETURNS TABLE (
  entry_id uuid,
  path text,
  heading text,
  content text,
  distance double precision,
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
SET hnsw.iterative_scan = 'relaxed_order'
AS $$
  WITH context AS NOT MATERIALIZED (
    SELECT brunn_auth.setting_uuid('app.current_user_id') AS user_id
    WHERE brunn_auth.context_is_valid()
  ), requested AS (
    SELECT CASE
             WHEN p_sort IN ('best_match','last_modified','title') THEN p_sort
             ELSE NULL
           END AS sort
  ), nearest_chunks AS MATERIALIZED (
    SELECT chunk.entry_id,chunk.path,chunk.heading,chunk.content,
           (
             chunk.embedding OPERATOR(public.<=>) p_embedding
           )::double precision AS distance
    FROM brunn.search_chunks AS chunk
    CROSS JOIN context
    WHERE chunk.user_id=context.user_id
      AND chunk.embedding IS NOT NULL
      AND chunk.path NOT LIKE '.brunn/checkpoints/%'
    ORDER BY chunk.embedding OPERATOR(public.<=>) p_embedding
    LIMIT 192
  ), nearest AS MATERIALIZED (
    SELECT DISTINCT ON (nearest_chunks.entry_id)
           nearest_chunks.entry_id,nearest_chunks.path,
           nearest_chunks.heading,nearest_chunks.content,
           nearest_chunks.distance
    FROM nearest_chunks
    ORDER BY nearest_chunks.entry_id,nearest_chunks.distance
  ), eligible AS MATERIALIZED (
    SELECT nearest.*,entry.title,entry.current_version,entry.updated_at,
           version.content_sha256
    FROM nearest
    CROSS JOIN context
    CROSS JOIN requested
    JOIN brunn.entries AS entry
      ON entry.user_id=context.user_id AND entry.id=nearest.entry_id
    JOIN brunn.entry_versions AS version
      ON version.user_id=entry.user_id
     AND version.entry_id=entry.id
     AND version.version=entry.current_version
    WHERE requested.sort IS NOT NULL
      AND entry.deleted_at IS NULL
  ), ranked AS MATERIALIZED (
    SELECT *
    FROM eligible
    ORDER BY
      CASE WHEN p_sort='best_match' THEN distance END ASC NULLS LAST,
      CASE WHEN p_sort='last_modified' THEN updated_at END DESC NULLS LAST,
      CASE WHEN p_sort='last_modified' THEN distance END ASC NULLS LAST,
      CASE WHEN p_sort='title' THEN lower(title) END ASC NULLS LAST,
      CASE WHEN p_sort='title' THEN updated_at END DESC NULLS LAST,
      updated_at DESC,path,entry_id
    LIMIT 64
  )
  SELECT ranked.entry_id,ranked.path,ranked.heading,ranked.content,
         ranked.distance,ranked.title,ranked.current_version,
         ranked.content_sha256,ranked.updated_at
  FROM ranked
  ORDER BY
    CASE WHEN p_sort='best_match' THEN ranked.distance END ASC NULLS LAST,
    CASE WHEN p_sort='last_modified' THEN ranked.updated_at END DESC NULLS LAST,
    CASE WHEN p_sort='last_modified' THEN ranked.distance END ASC NULLS LAST,
    CASE WHEN p_sort='title' THEN lower(ranked.title) END ASC NULLS LAST,
    CASE WHEN p_sort='title' THEN ranked.updated_at END DESC NULLS LAST,
    ranked.updated_at DESC,ranked.path,ranked.entry_id;
$$;

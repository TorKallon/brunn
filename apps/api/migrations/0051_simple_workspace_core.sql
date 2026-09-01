-- The owner-alpha schema remains in place for migration and rollback. New
-- workspace traffic uses this file/version/change model.

CREATE EXTENSION IF NOT EXISTS btree_gin;

CREATE TABLE brunn.entries (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES brunn.users(id),
  path text NOT NULL,
  title text NOT NULL,
  kind text NOT NULL CHECK (kind IN ('markdown', 'binary')),
  media_type text NOT NULL,
  current_version bigint NOT NULL DEFAULT 0 CHECK (current_version >= 0),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  deleted_at timestamptz,
  UNIQUE (user_id, id),
  CHECK (
    path <> ''
    AND path !~ '^/'
    AND path !~ '(^|/)\.\.?(/|$)'
    AND path !~ '[[:cntrl:]]'
  )
);

CREATE UNIQUE INDEX entries_user_path_unique_idx
  ON brunn.entries (user_id, lower(normalize(path, NFC)));

CREATE INDEX entries_user_updated_idx
  ON brunn.entries (user_id, updated_at DESC, id)
  WHERE deleted_at IS NULL;

CREATE INDEX entries_user_title_idx
  ON brunn.entries (user_id, lower(title))
  WHERE deleted_at IS NULL;

CREATE INDEX entries_user_manifest_idx
  ON brunn.entries (user_id, path, id)
  WHERE deleted_at IS NULL;

CREATE INDEX entries_user_history_manifest_idx
  ON brunn.entries (user_id, path, id);

CREATE TABLE brunn.entry_versions (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  entry_id uuid NOT NULL,
  version bigint NOT NULL CHECK (version > 0),
  content_sha256 brunn.sha256_hex NOT NULL,
  content text,
  object_key text,
  object_version_id text,
  object_etag text,
  size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  created_by_credential_id uuid,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, entry_id, version),
  FOREIGN KEY (user_id, entry_id)
    REFERENCES brunn.entries(user_id, id),
  FOREIGN KEY (user_id, created_by_credential_id)
    REFERENCES brunn.api_credentials(user_id, id),
  CHECK (
    (content IS NOT NULL AND object_key IS NULL AND object_version_id IS NULL)
    OR
    (content IS NULL AND object_key IS NOT NULL AND object_version_id IS NOT NULL)
  )
);

ALTER TABLE brunn.entries
  ADD CONSTRAINT entries_current_version_fk
  FOREIGN KEY (user_id, id, current_version)
  REFERENCES brunn.entry_versions(user_id, entry_id, version)
  DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX entry_versions_history_idx
  ON brunn.entry_versions (user_id, entry_id, version DESC);

CREATE TABLE brunn.workspace_changes (
  generation bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id uuid NOT NULL,
  entry_id uuid NOT NULL,
  entry_version bigint NOT NULL,
  operation text NOT NULL CHECK (operation IN ('create', 'update', 'delete')),
  path text NOT NULL,
  content_sha256 brunn.sha256_hex NOT NULL,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  FOREIGN KEY (user_id, entry_id, entry_version)
    REFERENCES brunn.entry_versions(user_id, entry_id, version)
);

CREATE INDEX workspace_changes_user_generation_idx
  ON brunn.workspace_changes (user_id, generation);

CREATE TABLE brunn.search_chunks (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  entry_id uuid NOT NULL,
  entry_version_id uuid NOT NULL,
  chunk_index integer NOT NULL CHECK (chunk_index >= 0),
  path text NOT NULL,
  heading text NOT NULL DEFAULT '',
  content text NOT NULL,
  token_estimate integer NOT NULL CHECK (token_estimate >= 0),
  embedding vector(1536),
  search_vector tsvector GENERATED ALWAYS AS (
    to_tsvector(
      'english',
      coalesce(path, '') || ' ' || coalesce(heading, '') || ' ' || coalesce(content, '')
    )
  ) STORED,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, entry_id, chunk_index),
  FOREIGN KEY (user_id, entry_id)
    REFERENCES brunn.entries(user_id, id)
    ON DELETE CASCADE,
  FOREIGN KEY (user_id, entry_version_id)
    REFERENCES brunn.entry_versions(user_id, id)
);

CREATE INDEX search_chunks_fts_idx
  ON brunn.search_chunks USING gin (user_id, search_vector);

CREATE INDEX search_chunks_path_idx
  ON brunn.search_chunks (user_id, lower(path), chunk_index);

CREATE INDEX search_chunks_embedding_hnsw_idx
  ON brunn.search_chunks
  USING hnsw (embedding vector_cosine_ops)
  WHERE embedding IS NOT NULL;

-- RLS remains the default defense-in-depth boundary for ordinary table reads.
-- Candidate ranking uses these context-bound functions because PostgreSQL's
-- security-barrier estimate otherwise prevents both GIN and HNSW plans. The
-- caller cannot provide a user ID: it comes only from the transaction context
-- already validated by the API.
CREATE FUNCTION brunn.workspace_lexical_candidates(
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
  content_sha256 brunn.sha256_hex
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
  ), requested AS (
    SELECT websearch_to_tsquery('english', p_query) AS query
  ), bounded_matches AS MATERIALIZED (
    SELECT chunk.id,chunk.entry_id,
           ts_rank_cd(chunk.search_vector,requested.query,32)::double precision AS score
    FROM brunn.search_chunks AS chunk
    CROSS JOIN context
    CROSS JOIN requested
    WHERE chunk.user_id=context.user_id
      AND chunk.search_vector @@ requested.query
      AND chunk.path NOT LIKE '.brunn/checkpoints/%'
    LIMIT 4096
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
  JOIN brunn.search_chunks AS chunk
    ON chunk.user_id=context.user_id AND chunk.id=ranked.id
  JOIN brunn.entries AS entry
    ON entry.user_id=context.user_id AND entry.id=ranked.entry_id
  JOIN brunn.entry_versions AS version
    ON version.user_id=entry.user_id
   AND version.entry_id=entry.id
   AND version.version=entry.current_version
  WHERE entry.deleted_at IS NULL
  ORDER BY ranked.entry_score DESC,chunk.path,ranked.section_rank
$$;

CREATE FUNCTION brunn.workspace_semantic_candidates(
  p_embedding vector(1536)
)
RETURNS TABLE (
  entry_id uuid,
  path text,
  heading text,
  content text,
  distance double precision,
  title text,
  current_version bigint,
  content_sha256 brunn.sha256_hex
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
SET hnsw.iterative_scan = 'relaxed_order'
AS $$
  WITH context AS (
    SELECT brunn_auth.setting_uuid('app.current_user_id') AS user_id
    WHERE brunn_auth.context_is_valid()
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
  ), ranked AS MATERIALIZED (
    SELECT *
    FROM nearest
    ORDER BY distance,entry_id
    LIMIT 48
  )
  SELECT ranked.entry_id,ranked.path,ranked.heading,ranked.content,
         ranked.distance,entry.title,entry.current_version,
         version.content_sha256
  FROM ranked
  JOIN context ON true
  JOIN brunn.entries AS entry
    ON entry.user_id=context.user_id AND entry.id=ranked.entry_id
  JOIN brunn.entry_versions AS version
    ON version.user_id=entry.user_id
   AND version.entry_id=entry.id
   AND version.version=entry.current_version
  WHERE entry.deleted_at IS NULL
  ORDER BY ranked.distance,ranked.path
$$;

REVOKE ALL ON FUNCTION
  brunn.workspace_lexical_candidates(text)
FROM PUBLIC;
REVOKE ALL ON FUNCTION
  brunn.workspace_semantic_candidates(vector)
FROM PUBLIC;
GRANT EXECUTE ON FUNCTION
  brunn.workspace_lexical_candidates(text)
TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION
  brunn.workspace_semantic_candidates(vector)
TO app_rw, app_ro;

CREATE TABLE brunn.jobs (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES brunn.users(id),
  kind text NOT NULL CHECK (kind IN (
    'embed_entry', 'describe_binary', 'dream_workspace'
  )),
  status text NOT NULL DEFAULT 'queued' CHECK (
    status IN ('queued', 'running', 'complete', 'failed')
  ),
  payload jsonb NOT NULL DEFAULT '{}'::jsonb,
  watermark bigint,
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  started_at timestamptz,
  finished_at timestamptz,
  last_error text,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE INDEX jobs_claim_idx
  ON brunn.jobs (status, available_at, created_at)
  WHERE status IN ('queued', 'running');

CREATE INDEX jobs_kind_claim_idx
  ON brunn.jobs (kind, status, available_at, created_at, id)
  WHERE status IN ('queued', 'running');

CREATE INDEX jobs_user_kind_status_idx
  ON brunn.jobs (user_id, kind, status);

CREATE TABLE brunn.entry_usage (
  user_id uuid NOT NULL,
  entry_id uuid NOT NULL,
  read_count bigint NOT NULL DEFAULT 0 CHECK (read_count >= 0),
  search_count bigint NOT NULL DEFAULT 0 CHECK (search_count >= 0),
  first_used_at timestamptz,
  last_used_at timestamptz,
  last_read_at timestamptz,
  last_search_at timestamptz,
  PRIMARY KEY (user_id, entry_id),
  FOREIGN KEY (user_id, entry_id)
    REFERENCES brunn.entries(user_id, id)
    ON DELETE CASCADE
);

DO $$
DECLARE
  table_name text;
BEGIN
  FOREACH table_name IN ARRAY ARRAY[
    'entries',
    'entry_versions',
    'workspace_changes',
    'search_chunks',
    'jobs',
    'entry_usage'
  ]
  LOOP
    EXECUTE format(
      'ALTER TABLE brunn.%I ENABLE ROW LEVEL SECURITY',
      table_name
    );
    EXECUTE format(
      'ALTER TABLE brunn.%I FORCE ROW LEVEL SECURITY',
      table_name
    );
    EXECUTE format(
      'CREATE POLICY simple_user_select ON brunn.%I '
      'FOR SELECT TO app_rw, app_ro '
      'USING (brunn_auth.can_access_user(user_id))',
      table_name
    );
    EXECUTE format(
      'CREATE POLICY simple_user_write ON brunn.%I '
      'FOR ALL TO app_rw '
      'USING ('
      '  brunn_auth.can_access_user(user_id) '
      '  AND brunn_auth.has_any_capability('
      '    ARRAY[''save'', ''checkpoint'', ''stage'', ''dream'', ''delete'']'
      '  )'
      ') '
      'WITH CHECK ('
      '  brunn_auth.can_access_user(user_id) '
      '  AND brunn_auth.has_any_capability('
      '    ARRAY[''save'', ''checkpoint'', ''stage'', ''dream'', ''delete'']'
      '  )'
      ')',
      table_name
    );
  END LOOP;
END;
$$;

GRANT SELECT, INSERT, UPDATE, DELETE ON
  brunn.entries,
  brunn.entry_versions,
  brunn.workspace_changes,
  brunn.search_chunks,
  brunn.jobs,
  brunn.entry_usage
TO app_rw;

GRANT SELECT ON
  brunn.entries,
  brunn.entry_versions,
  brunn.workspace_changes,
  brunn.search_chunks,
  brunn.jobs,
  brunn.entry_usage
TO app_ro;

GRANT USAGE, SELECT ON SEQUENCE brunn.workspace_changes_generation_seq
TO app_rw;

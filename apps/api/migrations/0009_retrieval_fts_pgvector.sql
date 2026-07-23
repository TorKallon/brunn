CREATE TABLE straylight.documents (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  object_id uuid,
  current_version integer NOT NULL DEFAULT 1 CHECK (current_version > 0),
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, object_id)
    REFERENCES straylight.objects(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version)
);

CREATE TRIGGER documents_register_record
BEFORE INSERT ON straylight.documents
FOR EACH ROW EXECUTE FUNCTION straylight.register_record_key('document');

CREATE TABLE straylight.document_revisions (
  user_id uuid NOT NULL,
  document_id uuid NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  previous_version integer,
  source_episode_id uuid NOT NULL,
  asset_id uuid,
  asset_version integer,
  title text,
  media_type straylight.nonempty_text NOT NULL,
  content_hash straylight.sha256_hex NOT NULL,
  native_locator jsonb NOT NULL DEFAULT '{}'::jsonb,
  byte_size bigint NOT NULL CHECK (byte_size >= 0),
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, document_id, version),
  FOREIGN KEY (user_id, document_id)
    REFERENCES straylight.documents(user_id, id),
  FOREIGN KEY (user_id, document_id, previous_version)
    REFERENCES straylight.document_revisions(user_id, document_id, version)
    DEFERRABLE INITIALLY DEFERRED,
  FOREIGN KEY (user_id, source_episode_id)
    REFERENCES straylight.source_episodes(user_id, id),
  FOREIGN KEY (user_id, asset_id, asset_version)
    REFERENCES straylight.asset_versions(user_id, asset_id, version),
  CHECK ((asset_id IS NULL) = (asset_version IS NULL)),
  CHECK (
    (version = 1 AND previous_version IS NULL)
    OR (version > 1 AND previous_version = version - 1)
  )
);

ALTER TABLE straylight.documents
  ADD CONSTRAINT documents_current_revision_fk
  FOREIGN KEY (user_id, id, current_version)
  REFERENCES straylight.document_revisions(user_id, document_id, version)
  DEFERRABLE INITIALLY DEFERRED;

CREATE FUNCTION straylight.build_chunk_search_vector(
  p_heading_path text[],
  p_content text
)
RETURNS tsvector
LANGUAGE sql
IMMUTABLE
PARALLEL SAFE
AS $$
  SELECT
    setweight(
      to_tsvector('english'::regconfig, coalesce(array_to_string(p_heading_path, ' '), '')),
      'A'
    ) ||
    setweight(to_tsvector('english'::regconfig, coalesce(p_content, '')), 'B')
$$;

CREATE TABLE straylight.chunks (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  document_id uuid NOT NULL,
  document_version integer NOT NULL,
  parent_chunk_id uuid,
  ordinal integer NOT NULL CHECK (ordinal >= 0),
  chunk_kind straylight.nonempty_text NOT NULL,
  heading_path text[] NOT NULL DEFAULT '{}'::text[],
  locator jsonb NOT NULL,
  content text NOT NULL,
  content_hash straylight.sha256_hex NOT NULL,
  token_count integer NOT NULL CHECK (token_count >= 0),
  search_vector tsvector GENERATED ALWAYS AS (
    straylight.build_chunk_search_vector(heading_path, content)
  ) STORED,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, document_id, document_version, ordinal),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, document_id, document_version)
    REFERENCES straylight.document_revisions(user_id, document_id, version),
  FOREIGN KEY (user_id, parent_chunk_id)
    REFERENCES straylight.chunks(user_id, id)
);

CREATE TRIGGER chunks_register_record
BEFORE INSERT ON straylight.chunks
FOR EACH ROW EXECUTE FUNCTION straylight.register_record_key('chunk');

CREATE INDEX chunks_fts_idx ON straylight.chunks USING gin (search_vector);
CREATE INDEX chunks_scope_order_idx
  ON straylight.chunks (user_id, scope_id, document_id, document_version, ordinal);

CREATE TABLE straylight.embeddings (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  target_record_id uuid NOT NULL,
  granularity text NOT NULL CHECK (granularity IN ('object', 'artifact', 'section', 'chunk')),
  model straylight.nonempty_text NOT NULL,
  dimensions integer NOT NULL DEFAULT 1536 CHECK (dimensions = 1536),
  embedding vector(1536) NOT NULL,
  source_content_hash straylight.sha256_hex NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, target_record_id, granularity, model, source_content_hash),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id),
  CHECK (vector_dims(embedding) = dimensions)
);

CREATE INDEX embeddings_hnsw_cosine_idx
  ON straylight.embeddings USING hnsw (embedding vector_cosine_ops)
  WITH (m = 16, ef_construction = 128);

CREATE INDEX embeddings_scope_model_idx
  ON straylight.embeddings (user_id, scope_id, model, granularity);

CREATE TABLE straylight.retrieval_operations (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  session_id uuid NOT NULL,
  corpus_revision_id uuid NOT NULL,
  request_id straylight.nonempty_text NOT NULL,
  operation_kind text NOT NULL CHECK (operation_kind IN (
    'open', 'query', 'read', 'compute', 'verify', 'status'
  )),
  request_hash straylight.sha256_hex NOT NULL,
  status text NOT NULL CHECK (status IN (
    'complete', 'partial', 'ambiguous', 'stale', 'inconsistent', 'degraded', 'failed'
  )),
  coverage jsonb NOT NULL DEFAULT '{}'::jsonb,
  metrics jsonb NOT NULL DEFAULT '{}'::jsonb,
  started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  finished_at timestamptz,
  UNIQUE (user_id, id),
  UNIQUE (user_id, request_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, session_id)
    REFERENCES straylight.sessions(user_id, id),
  FOREIGN KEY (user_id, corpus_revision_id)
    REFERENCES straylight.corpus_revisions(user_id, id),
  CHECK (finished_at IS NULL OR finished_at >= started_at)
);

CREATE TABLE straylight.retrieval_result_items (
  user_id uuid NOT NULL,
  operation_id uuid NOT NULL,
  item_id straylight.nonempty_text NOT NULL,
  target_record_id uuid,
  result_order integer NOT NULL CHECK (result_order >= 0),
  lane_scores jsonb NOT NULL DEFAULT '{}'::jsonb,
  why_selected jsonb NOT NULL DEFAULT '[]'::jsonb,
  evidence_refs uuid[] NOT NULL DEFAULT '{}'::uuid[],
  payload jsonb NOT NULL,
  PRIMARY KEY (user_id, operation_id, item_id),
  FOREIGN KEY (user_id, operation_id)
    REFERENCES straylight.retrieval_operations(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id),
  UNIQUE (user_id, operation_id, result_order)
);

CREATE TABLE straylight.continuation_tokens (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  session_id uuid NOT NULL,
  corpus_revision_id uuid NOT NULL,
  operation_kind straylight.nonempty_text NOT NULL,
  query_hash straylight.sha256_hex NOT NULL,
  token_hash straylight.sha256_hex NOT NULL,
  next_locator jsonb NOT NULL,
  expires_at timestamptz NOT NULL,
  consumed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, token_hash),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, session_id)
    REFERENCES straylight.sessions(user_id, id),
  FOREIGN KEY (user_id, corpus_revision_id)
    REFERENCES straylight.corpus_revisions(user_id, id),
  CHECK (expires_at > created_at)
);

CREATE TABLE straylight.materialization_cache (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  corpus_revision_id uuid NOT NULL,
  credential_projection_hash straylight.sha256_hex NOT NULL,
  root_set_hash straylight.sha256_hex NOT NULL,
  policy_version_hash straylight.sha256_hex NOT NULL,
  audience straylight.nonempty_text NOT NULL,
  purpose straylight.nonempty_text NOT NULL,
  view straylight.nonempty_text NOT NULL,
  tokenizer_version straylight.nonempty_text NOT NULL,
  payload jsonb NOT NULL,
  token_count integer NOT NULL CHECK (token_count >= 0),
  expires_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (
    user_id, corpus_revision_id, credential_projection_hash, root_set_hash,
    policy_version_hash, audience, purpose, view, tokenizer_version
  ),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, corpus_revision_id)
    REFERENCES straylight.corpus_revisions(user_id, id),
  CHECK (expires_at > created_at)
);

CREATE TRIGGER document_revisions_immutable
BEFORE UPDATE OR DELETE ON straylight.document_revisions
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER chunks_immutable
BEFORE UPDATE OR DELETE ON straylight.chunks
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER retrieval_result_items_immutable
BEFORE UPDATE OR DELETE ON straylight.retrieval_result_items
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER documents_head_advance
BEFORE UPDATE OF current_version ON straylight.documents
FOR EACH ROW EXECUTE FUNCTION straylight.enforce_sequential_head_advance();

CREATE TRIGGER documents_no_delete
BEFORE DELETE ON straylight.documents
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_physical_delete();

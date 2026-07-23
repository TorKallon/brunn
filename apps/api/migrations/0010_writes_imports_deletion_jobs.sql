CREATE TABLE straylight.write_operations (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  credential_id uuid NOT NULL,
  operation_kind text NOT NULL CHECK (operation_kind IN (
    'save', 'checkpoint', 'stage', 'correct', 'delete', 'import_promotion'
  )),
  intent straylight.nonempty_text NOT NULL,
  request_hash straylight.sha256_hex NOT NULL,
  base_corpus_revision_id uuid NOT NULL,
  result_corpus_revision_id uuid,
  status text NOT NULL CHECK (status IN (
    'pending', 'committed', 'no_op', 'accepted_processing', 'needs_review',
    'conflict', 'rejected_by_policy', 'failed'
  )),
  receipt jsonb,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  completed_at timestamptz,
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  FOREIGN KEY (user_id, base_corpus_revision_id)
    REFERENCES straylight.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, result_corpus_revision_id)
    REFERENCES straylight.corpus_revisions(user_id, id),
  CHECK (completed_at IS NULL OR completed_at >= created_at),
  CHECK ((status = 'pending') = (completed_at IS NULL))
);

CREATE TABLE straylight.idempotency_keys (
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  idempotency_key straylight.nonempty_text NOT NULL,
  request_hash straylight.sha256_hex NOT NULL,
  operation_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, scope_id, idempotency_key),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, operation_id)
    REFERENCES straylight.write_operations(user_id, id)
);

CREATE TABLE straylight.write_operation_items (
  user_id uuid NOT NULL,
  operation_id uuid NOT NULL,
  item_order integer NOT NULL CHECK (item_order >= 0),
  action text NOT NULL CHECK (action IN (
    'create', 'revise', 'supersede', 'retract', 'relate', 'tombstone'
  )),
  item_kind straylight.nonempty_text NOT NULL,
  target_record_id uuid,
  prior_version integer,
  resulting_version integer,
  disposition text NOT NULL CHECK (disposition IN (
    'created', 'revised', 'superseded', 'retracted', 'deduplicated',
    'rejected', 'needs_review', 'tombstoned'
  )),
  details jsonb NOT NULL DEFAULT '{}'::jsonb,
  PRIMARY KEY (user_id, operation_id, item_order),
  FOREIGN KEY (user_id, operation_id)
    REFERENCES straylight.write_operations(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id),
  CHECK (prior_version IS NULL OR prior_version > 0),
  CHECK (resulting_version IS NULL OR resulting_version > 0)
);

CREATE TABLE straylight.stages (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  credential_id uuid NOT NULL,
  stable_import_id text,
  base_corpus_revision_id uuid NOT NULL,
  input_hash straylight.sha256_hex NOT NULL,
  inventory_hash straylight.sha256_hex,
  status text NOT NULL CHECK (status IN (
    'uploading', 'inspecting', 'ready', 'quarantined', 'expired', 'promoted', 'failed'
  )),
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  expires_at timestamptz NOT NULL,
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  FOREIGN KEY (user_id, base_corpus_revision_id)
    REFERENCES straylight.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version),
  CHECK (expires_at > created_at)
);

CREATE TRIGGER stages_register_record
BEFORE INSERT ON straylight.stages
FOR EACH ROW EXECUTE FUNCTION straylight.register_record_key('stage');

CREATE TABLE straylight.staged_entries (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  stage_id uuid NOT NULL,
  path straylight.nonempty_text NOT NULL,
  entry_kind text NOT NULL CHECK (entry_kind IN (
    'file', 'directory', 'archive', 'symlink', 'unreadable'
  )),
  media_type text,
  size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
  content_hash straylight.sha256_hex,
  asset_id uuid,
  asset_version integer,
  readability text NOT NULL CHECK (readability IN (
    'readable', 'unsupported', 'unreadable', 'quarantined'
  )),
  inspection jsonb NOT NULL DEFAULT '{}'::jsonb,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, stage_id, path),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, stage_id)
    REFERENCES straylight.stages(user_id, id),
  FOREIGN KEY (user_id, asset_id, asset_version)
    REFERENCES straylight.asset_versions(user_id, asset_id, version),
  CHECK ((asset_id IS NULL) = (asset_version IS NULL)),
  CHECK (path !~ '(^|/)\.\.(/|$)' AND left(path, 1) <> '/')
);

CREATE TABLE straylight.import_receipts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  stable_import_id straylight.nonempty_text NOT NULL,
  stage_id uuid NOT NULL,
  input_hash straylight.sha256_hex NOT NULL,
  inventory_hash straylight.sha256_hex NOT NULL,
  manifest_hash straylight.sha256_hex NOT NULL,
  base_corpus_revision_id uuid NOT NULL,
  result_corpus_revision_id uuid NOT NULL,
  replay_outcome text NOT NULL CHECK (replay_outcome IN (
    'new_import', 'exact_replay', 'revision_delta'
  )),
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  indexing_state jsonb NOT NULL,
  receipt jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (
    user_id, scope_id, stable_import_id, input_hash, base_corpus_revision_id
  ),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, stage_id)
    REFERENCES straylight.stages(user_id, id),
  FOREIGN KEY (user_id, base_corpus_revision_id)
    REFERENCES straylight.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, result_corpus_revision_id)
    REFERENCES straylight.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version)
);

CREATE TABLE straylight.import_entry_dispositions (
  user_id uuid NOT NULL,
  import_receipt_id uuid NOT NULL,
  staged_entry_id uuid NOT NULL,
  disposition text NOT NULL CHECK (disposition IN (
    'create_artifact', 'retain_as_source', 'duplicate_link', 'committed',
    'linked', 'deduplicated', 'skipped', 'unreadable', 'unresolved',
    'rejected', 'quarantined'
  )),
  resulting_record_id uuid,
  details jsonb NOT NULL DEFAULT '{}'::jsonb,
  PRIMARY KEY (user_id, import_receipt_id, staged_entry_id),
  FOREIGN KEY (user_id, import_receipt_id)
    REFERENCES straylight.import_receipts(user_id, id),
  FOREIGN KEY (user_id, staged_entry_id)
    REFERENCES straylight.staged_entries(user_id, id),
  FOREIGN KEY (user_id, resulting_record_id)
    REFERENCES straylight.record_keys(user_id, record_id)
);

CREATE TABLE straylight.background_jobs (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  job_kind straylight.nonempty_text NOT NULL,
  status text NOT NULL CHECK (status IN (
    'queued', 'running', 'retry_wait', 'succeeded', 'failed', 'canceled'
  )),
  payload jsonb NOT NULL,
  result jsonb,
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts integer NOT NULL DEFAULT 5 CHECK (max_attempts > 0),
  available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  locked_at timestamptz,
  locked_by text,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  completed_at timestamptz,
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  CHECK (attempts <= max_attempts),
  CHECK (completed_at IS NULL OR completed_at >= created_at)
);

CREATE INDEX background_jobs_claim_idx
  ON straylight.background_jobs (status, available_at, created_at)
  WHERE status IN ('queued', 'retry_wait');

CREATE TABLE straylight.tombstones (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  target_record_id uuid NOT NULL,
  source_episode_id uuid NOT NULL,
  evidence_id uuid NOT NULL,
  authorized_credential_id uuid NOT NULL,
  reason straylight.nonempty_text NOT NULL,
  requested_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, target_record_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id),
  FOREIGN KEY (user_id, source_episode_id)
    REFERENCES straylight.source_episodes(user_id, id),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES straylight.evidence_items(user_id, id),
  FOREIGN KEY (user_id, authorized_credential_id)
    REFERENCES straylight.api_credentials(user_id, id)
);

CREATE TRIGGER tombstones_register_record
BEFORE INSERT ON straylight.tombstones
FOR EACH ROW EXECUTE FUNCTION straylight.register_record_key('tombstone');

CREATE TABLE straylight.deletion_jobs (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  tombstone_id uuid NOT NULL,
  status text NOT NULL CHECK (status IN (
    'queued', 'propagating', 'blocked', 'completed', 'failed'
  )),
  started_at timestamptz,
  completed_at timestamptz,
  terminal_result jsonb,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, tombstone_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, tombstone_id)
    REFERENCES straylight.tombstones(user_id, id),
  CHECK (completed_at IS NULL OR completed_at >= coalesce(started_at, created_at))
);

CREATE TABLE straylight.derivative_cleanup_targets (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  deletion_job_id uuid NOT NULL,
  target_kind text NOT NULL CHECK (target_kind IN (
    'source', 'chunk', 'lexical_index', 'embedding', 'cache', 'export',
    'replica', 'asset', 'derived_view'
  )),
  target_ref straylight.nonempty_text NOT NULL,
  status text NOT NULL CHECK (status IN (
    'pending', 'running', 'removed', 'retained_by_policy', 'failed'
  )),
  attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
  details jsonb NOT NULL DEFAULT '{}'::jsonb,
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, deletion_job_id, target_kind, target_ref),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, deletion_job_id)
    REFERENCES straylight.deletion_jobs(user_id, id)
);

CREATE TABLE straylight.deletion_job_events (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  deletion_job_id uuid NOT NULL,
  event_kind straylight.nonempty_text NOT NULL,
  details jsonb NOT NULL DEFAULT '{}'::jsonb,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, deletion_job_id)
    REFERENCES straylight.deletion_jobs(user_id, id)
);

CREATE TRIGGER import_receipts_immutable
BEFORE UPDATE OR DELETE ON straylight.import_receipts
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER import_entry_dispositions_immutable
BEFORE UPDATE OR DELETE ON straylight.import_entry_dispositions
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER tombstones_immutable
BEFORE UPDATE OR DELETE ON straylight.tombstones
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER deletion_job_events_immutable
BEFORE UPDATE OR DELETE ON straylight.deletion_job_events
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();


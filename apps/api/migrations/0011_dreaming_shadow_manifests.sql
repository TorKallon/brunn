CREATE TABLE brunn.dream_regions (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  region_ref brunn.nonempty_text NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  root_set_hash brunn.sha256_hex NOT NULL,
  manifest_hash brunn.sha256_hex NOT NULL,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, scope_id, region_ref, version),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES brunn.policy_revisions(user_id, policy_id, version)
);

CREATE TABLE brunn.dream_region_members (
  user_id uuid NOT NULL,
  region_id uuid NOT NULL,
  record_id uuid NOT NULL,
  expansion_reason brunn.nonempty_text NOT NULL,
  PRIMARY KEY (user_id, region_id, record_id),
  FOREIGN KEY (user_id, region_id)
    REFERENCES brunn.dream_regions(user_id, id),
  FOREIGN KEY (user_id, record_id)
    REFERENCES brunn.record_keys(user_id, record_id)
);

CREATE TABLE brunn.dream_jobs (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  job_type text NOT NULL CHECK (job_type IN (
    'deterministic_maintenance', 'deep_consolidation', 'repair_only'
  )),
  trigger_reasons text[] NOT NULL,
  status text NOT NULL CHECK (status IN (
    'queued', 'selecting', 'opening_snapshot', 'generating', 'gating',
    'evaluating', 'awaiting_review', 'promoting', 'monitoring',
    'promoted', 'quarantined', 'discarded', 'failed', 'canceled',
    'stale', 'rolled_back'
  )),
  base_corpus_revision_id uuid NOT NULL,
  evidence_watermark brunn.nonempty_text NOT NULL,
  region_id uuid NOT NULL,
  dream_policy_version brunn.nonempty_text NOT NULL,
  model_version brunn.nonempty_text NOT NULL,
  prompt_version brunn.nonempty_text NOT NULL,
  schema_version brunn.nonempty_text NOT NULL,
  evaluator_version brunn.nonempty_text NOT NULL,
  compute_budget jsonb NOT NULL,
  requested_by_credential_id uuid,
  started_at timestamptz,
  completed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, base_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, region_id)
    REFERENCES brunn.dream_regions(user_id, id),
  FOREIGN KEY (user_id, requested_by_credential_id)
    REFERENCES brunn.api_credentials(user_id, id),
  CHECK (completed_at IS NULL OR completed_at >= coalesce(started_at, created_at))
);

CREATE TRIGGER dream_jobs_register_record
BEFORE INSERT ON brunn.dream_jobs
FOR EACH ROW EXECUTE FUNCTION brunn.register_record_key('dream_job');

CREATE TABLE brunn.dream_job_events (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  dream_job_id uuid NOT NULL,
  from_status text,
  to_status brunn.nonempty_text NOT NULL,
  details jsonb NOT NULL DEFAULT '{}'::jsonb,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, dream_job_id)
    REFERENCES brunn.dream_jobs(user_id, id)
);

CREATE TABLE brunn.dream_candidate_revisions (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  dream_job_id uuid NOT NULL,
  base_corpus_revision_id uuid NOT NULL,
  candidate_corpus_revision_id uuid NOT NULL,
  candidate_manifest_hash brunn.sha256_hex NOT NULL,
  status text NOT NULL CHECK (status IN (
    'building', 'ready', 'gated', 'quarantined', 'promoted', 'discarded', 'stale'
  )),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, dream_job_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, dream_job_id)
    REFERENCES brunn.dream_jobs(user_id, id),
  FOREIGN KEY (user_id, base_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, candidate_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  CHECK (candidate_corpus_revision_id <> base_corpus_revision_id)
);

CREATE TABLE brunn.dream_candidate_items (
  user_id uuid NOT NULL,
  candidate_revision_id uuid NOT NULL,
  target_record_id uuid NOT NULL,
  disposition text NOT NULL CHECK (disposition IN (
    'keep', 'add_derived_view', 'add_alias', 'add_soft_link', 'add_cluster',
    'add_flag', 'propose_same_as', 'propose_supersession',
    'propose_scope_move', 'propose_tombstone'
  )),
  requires_review boolean NOT NULL,
  source_refs uuid[] NOT NULL,
  proposal jsonb NOT NULL,
  PRIMARY KEY (user_id, candidate_revision_id, target_record_id),
  FOREIGN KEY (user_id, candidate_revision_id)
    REFERENCES brunn.dream_candidate_revisions(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES brunn.record_keys(user_id, record_id),
  CHECK (cardinality(source_refs) > 0)
);

CREATE TABLE brunn.dream_gate_results (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  candidate_revision_id uuid NOT NULL,
  gate_ref brunn.nonempty_text NOT NULL,
  gate_kind text NOT NULL CHECK (gate_kind IN (
    'reference', 'hash', 'temporal', 'policy', 'lineage', 'state_machine',
    'canonical_head', 'preservation', 'retrieval_regression', 'task_family'
  )),
  hard_gate boolean NOT NULL,
  passed boolean NOT NULL,
  evidence_refs uuid[] NOT NULL DEFAULT '{}'::uuid[],
  details jsonb NOT NULL,
  evaluated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, candidate_revision_id, gate_ref),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, candidate_revision_id)
    REFERENCES brunn.dream_candidate_revisions(user_id, id)
);

CREATE TABLE brunn.dream_evaluations (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  candidate_revision_id uuid NOT NULL,
  active_corpus_revision_id uuid NOT NULL,
  frozen_suite_hash brunn.sha256_hex NOT NULL,
  reader_version brunn.nonempty_text NOT NULL,
  retrieval_policy_version brunn.nonempty_text NOT NULL,
  result jsonb NOT NULL,
  passed boolean NOT NULL,
  evaluated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, candidate_revision_id)
    REFERENCES brunn.dream_candidate_revisions(user_id, id),
  FOREIGN KEY (user_id, active_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id)
);

CREATE TABLE brunn.dream_reviews (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  candidate_revision_id uuid NOT NULL,
  target_record_id uuid,
  reviewer_ref uuid NOT NULL,
  decision text NOT NULL CHECK (decision IN ('approved', 'rejected', 'changes_requested')),
  reason brunn.nonempty_text NOT NULL,
  reviewed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, candidate_revision_id)
    REFERENCES brunn.dream_candidate_revisions(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES brunn.record_keys(user_id, record_id),
  FOREIGN KEY (user_id, reviewer_ref)
    REFERENCES brunn.record_keys(user_id, record_id)
);

CREATE TABLE brunn.active_manifests (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  active_corpus_revision_id uuid NOT NULL,
  manifest_hash brunn.sha256_hex NOT NULL,
  generation bigint NOT NULL DEFAULT 1 CHECK (generation > 0),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, scope_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, active_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id)
);

CREATE FUNCTION brunn.enforce_manifest_compare_and_swap()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NEW.user_id <> OLD.user_id OR NEW.scope_id <> OLD.scope_id OR NEW.id <> OLD.id THEN
    RAISE EXCEPTION 'manifest ownership and identity are immutable'
      USING ERRCODE = '55000';
  END IF;
  IF NEW.generation <> OLD.generation + 1 THEN
    RAISE EXCEPTION 'manifest generation compare-and-swap failed'
      USING ERRCODE = '40001';
  END IF;
  IF NEW.active_corpus_revision_id = OLD.active_corpus_revision_id THEN
    RAISE EXCEPTION 'manifest update must change the active revision'
      USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER active_manifests_compare_and_swap
BEFORE UPDATE ON brunn.active_manifests
FOR EACH ROW EXECUTE FUNCTION brunn.enforce_manifest_compare_and_swap();

CREATE TRIGGER active_manifests_no_delete
BEFORE DELETE ON brunn.active_manifests
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_physical_delete();

CREATE TABLE brunn.active_manifest_history (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  manifest_id uuid NOT NULL,
  generation bigint NOT NULL CHECK (generation > 0),
  corpus_revision_id uuid NOT NULL,
  manifest_hash brunn.sha256_hex NOT NULL,
  change_kind text NOT NULL CHECK (change_kind IN (
    'initial', 'write', 'promotion', 'rollback'
  )),
  dream_job_id uuid,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, manifest_id, generation),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, manifest_id)
    REFERENCES brunn.active_manifests(user_id, id),
  FOREIGN KEY (user_id, corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, dream_job_id)
    REFERENCES brunn.dream_jobs(user_id, id)
);

CREATE TABLE brunn.dream_promotion_receipts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  dream_job_id uuid NOT NULL,
  candidate_revision_id uuid NOT NULL,
  previous_corpus_revision_id uuid NOT NULL,
  promoted_corpus_revision_id uuid NOT NULL,
  manifest_generation bigint NOT NULL CHECK (manifest_generation > 0),
  gate_result_ids uuid[] NOT NULL,
  review_ids uuid[] NOT NULL DEFAULT '{}'::uuid[],
  promoted_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, dream_job_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, dream_job_id)
    REFERENCES brunn.dream_jobs(user_id, id),
  FOREIGN KEY (user_id, candidate_revision_id)
    REFERENCES brunn.dream_candidate_revisions(user_id, id),
  FOREIGN KEY (user_id, previous_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, promoted_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id)
);

CREATE TABLE brunn.dream_rollback_receipts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  promotion_receipt_id uuid NOT NULL,
  failed_corpus_revision_id uuid NOT NULL,
  restored_corpus_revision_id uuid NOT NULL,
  manifest_generation bigint NOT NULL CHECK (manifest_generation > 0),
  reason brunn.nonempty_text NOT NULL,
  rolled_back_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, promotion_receipt_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, promotion_receipt_id)
    REFERENCES brunn.dream_promotion_receipts(user_id, id),
  FOREIGN KEY (user_id, failed_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, restored_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id)
);

CREATE TABLE brunn.dream_canary_observations (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  promotion_receipt_id uuid NOT NULL,
  request_hash brunn.sha256_hex NOT NULL,
  old_selection jsonb NOT NULL,
  new_selection jsonb NOT NULL,
  correction jsonb,
  source_invalidations uuid[] NOT NULL DEFAULT '{}'::uuid[],
  observed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, promotion_receipt_id)
    REFERENCES brunn.dream_promotion_receipts(user_id, id)
);

CREATE TABLE brunn.dream_region_locks (
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  region_id uuid NOT NULL,
  dream_job_id uuid NOT NULL,
  locked_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  expires_at timestamptz NOT NULL,
  PRIMARY KEY (user_id, region_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, region_id)
    REFERENCES brunn.dream_regions(user_id, id),
  FOREIGN KEY (user_id, dream_job_id)
    REFERENCES brunn.dream_jobs(user_id, id),
  CHECK (expires_at > locked_at)
);

DO $$
DECLARE
  table_name text;
BEGIN
  FOREACH table_name IN ARRAY ARRAY[
    'dream_regions', 'dream_region_members', 'dream_job_events',
    'dream_candidate_items', 'dream_gate_results', 'dream_evaluations',
    'dream_reviews', 'active_manifest_history', 'dream_promotion_receipts',
    'dream_rollback_receipts', 'dream_canary_observations'
  ]
  LOOP
    EXECUTE format(
      'CREATE TRIGGER %I_immutable BEFORE UPDATE OR DELETE ON brunn.%I '
      'FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation()',
      table_name, table_name
    );
  END LOOP;
END;
$$;

CREATE TRIGGER dream_candidate_revisions_immutable
BEFORE UPDATE OR DELETE ON brunn.dream_candidate_revisions
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

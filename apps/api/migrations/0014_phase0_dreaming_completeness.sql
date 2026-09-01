CREATE TABLE brunn.corpus_revision_counters (
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  last_revision_number bigint NOT NULL CHECK (last_revision_number > 0),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, scope_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id)
);

INSERT INTO brunn.corpus_revision_counters (
  user_id,scope_id,last_revision_number
)
SELECT user_id,scope_id,max(revision_number)
FROM brunn.corpus_revisions
GROUP BY user_id,scope_id;

CREATE FUNCTION brunn.allocate_corpus_revision_number()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  INSERT INTO brunn.corpus_revision_counters AS counter (
    user_id,scope_id,last_revision_number
  ) VALUES (
    NEW.user_id,
    NEW.scope_id,
    coalesce((
      SELECT max(revision.revision_number) + 1
      FROM brunn.corpus_revisions AS revision
      WHERE revision.user_id=NEW.user_id AND revision.scope_id=NEW.scope_id
    ),1)
  )
  ON CONFLICT (user_id,scope_id) DO UPDATE
    SET last_revision_number=counter.last_revision_number+1,
        updated_at=clock_timestamp()
  RETURNING last_revision_number INTO NEW.revision_number;
  RETURN NEW;
END;
$$;

CREATE TRIGGER corpus_revisions_allocate_number
BEFORE INSERT ON brunn.corpus_revisions
FOR EACH ROW EXECUTE FUNCTION brunn.allocate_corpus_revision_number();

CREATE TABLE brunn.dream_model_receipts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  dream_job_id uuid NOT NULL,
  provider brunn.nonempty_text NOT NULL,
  requested_model brunn.nonempty_text NOT NULL,
  returned_model text,
  response_id text,
  request_hash brunn.sha256_hex NOT NULL,
  prompt_version brunn.nonempty_text NOT NULL,
  status text NOT NULL CHECK (status IN (
    'completed', 'not_requested', 'degraded_unavailable', 'refused',
    'invalid_output'
  )),
  output jsonb NOT NULL DEFAULT '{}'::jsonb,
  validation jsonb NOT NULL DEFAULT '{}'::jsonb,
  usage jsonb NOT NULL DEFAULT '{}'::jsonb,
  degradation_reason text,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, dream_job_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, dream_job_id)
    REFERENCES brunn.dream_jobs(user_id, id),
  CHECK (
    (status = 'completed' AND degradation_reason IS NULL)
    OR (status <> 'completed')
  )
);

CREATE TRIGGER dream_model_receipts_immutable
BEFORE UPDATE OR DELETE ON brunn.dream_model_receipts
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TABLE brunn.dream_scheduler_controls (
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  enabled boolean NOT NULL DEFAULT true,
  paused boolean NOT NULL DEFAULT false,
  repair_only boolean NOT NULL DEFAULT false,
  dirty_score integer NOT NULL DEFAULT 0 CHECK (dirty_score >= 0),
  dirty_reasons text[] NOT NULL DEFAULT '{}'::text[],
  dirty_root_ids uuid[] NOT NULL DEFAULT '{}'::uuid[],
  dirty_since timestamptz,
  observed_corpus_revision_id uuid,
  consolidated_corpus_revision_id uuid,
  last_scheduled_at timestamptz,
  last_completed_at timestamptz,
  cooldown_until timestamptz,
  updated_by_credential_id uuid,
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, scope_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, observed_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, consolidated_corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, updated_by_credential_id)
    REFERENCES brunn.api_credentials(user_id, id),
  CHECK (array_position(dirty_reasons, NULL) IS NULL),
  CHECK (array_position(dirty_root_ids, NULL) IS NULL),
  CHECK (cardinality(dirty_reasons) <= 64),
  CHECK (cardinality(dirty_root_ids) <= 256)
);

CREATE INDEX dream_scheduler_eligibility_idx
  ON brunn.dream_scheduler_controls (
    enabled, paused, cooldown_until, dirty_score, updated_at
  );

ALTER TABLE brunn.dream_model_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.dream_model_receipts FORCE ROW LEVEL SECURITY;
ALTER TABLE brunn.dream_scheduler_controls ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.dream_scheduler_controls FORCE ROW LEVEL SECURITY;
ALTER TABLE brunn.corpus_revision_counters ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.corpus_revision_counters FORCE ROW LEVEL SECURITY;

CREATE POLICY scope_select ON brunn.corpus_revision_counters
  FOR SELECT TO app_rw, app_ro
  USING (brunn_auth.can_access_scope(user_id, scope_id));

CREATE POLICY scope_write ON brunn.corpus_revision_counters
  FOR ALL TO app_rw
  USING (
    brunn_auth.can_access_scope(user_id, scope_id)
    AND brunn_auth.has_any_capability(ARRAY[
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream'
    ])
  )
  WITH CHECK (
    brunn_auth.can_access_scope(user_id, scope_id)
    AND brunn_auth.has_any_capability(ARRAY[
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream'
    ])
  );

CREATE POLICY scope_select ON brunn.dream_model_receipts
  FOR SELECT TO app_rw, app_ro
  USING (brunn_auth.can_access_scope(user_id, scope_id));

CREATE POLICY scope_write ON brunn.dream_model_receipts
  FOR ALL TO app_rw
  USING (
    brunn_auth.can_access_scope(user_id, scope_id)
    AND brunn_auth.has_capability('dream')
  )
  WITH CHECK (
    brunn_auth.can_access_scope(user_id, scope_id)
    AND brunn_auth.has_capability('dream')
  );

CREATE POLICY scope_select ON brunn.dream_scheduler_controls
  FOR SELECT TO app_rw, app_ro
  USING (brunn_auth.can_access_scope(user_id, scope_id));

CREATE POLICY scope_write ON brunn.dream_scheduler_controls
  FOR ALL TO app_rw
  USING (
    brunn_auth.can_access_scope(user_id, scope_id)
    AND brunn_auth.has_capability('dream')
  )
  WITH CHECK (
    brunn_auth.can_access_scope(user_id, scope_id)
    AND brunn_auth.has_capability('dream')
  );

GRANT SELECT, INSERT ON brunn.dream_model_receipts TO app_rw;
GRANT SELECT, INSERT, UPDATE, DELETE ON brunn.dream_scheduler_controls TO app_rw;
GRANT SELECT, INSERT, UPDATE ON brunn.corpus_revision_counters TO app_rw;
GRANT SELECT ON
  brunn.corpus_revision_counters,
  brunn.dream_model_receipts,
  brunn.dream_scheduler_controls
TO app_ro;

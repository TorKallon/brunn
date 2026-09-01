ALTER TABLE brunn.record_keys
  ADD CONSTRAINT record_keys_kind_unique
  UNIQUE (user_id, record_id, record_kind);

CREATE TABLE brunn.corpus_revisions (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  parent_revision_id uuid,
  revision_number bigint NOT NULL CHECK (revision_number > 0),
  manifest_hash brunn.sha256_hex NOT NULL,
  operation_id uuid,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, scope_id, revision_number),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, parent_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  CHECK (parent_revision_id IS NOT NULL OR revision_number = 1)
);

CREATE TRIGGER corpus_revisions_register_record
BEFORE INSERT ON brunn.corpus_revisions
FOR EACH ROW EXECUTE FUNCTION brunn.register_record_key('corpus_revision');

CREATE TABLE brunn.corpus_members (
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  corpus_revision_id uuid NOT NULL,
  record_id uuid NOT NULL,
  record_kind brunn.nonempty_text NOT NULL,
  record_version integer,
  disposition text NOT NULL DEFAULT 'active' CHECK (
    disposition IN ('active', 'superseded', 'retracted', 'tombstoned')
  ),
  PRIMARY KEY (user_id, corpus_revision_id, record_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, record_id, record_kind)
    REFERENCES brunn.record_keys(user_id, record_id, record_kind),
  CHECK (record_version IS NULL OR record_version > 0)
);

CREATE INDEX corpus_members_kind_idx
  ON brunn.corpus_members (
    user_id, scope_id, corpus_revision_id, record_kind, disposition
  );

CREATE TABLE brunn.sessions (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  credential_id uuid NOT NULL,
  corpus_revision_id uuid NOT NULL,
  parent_session_id uuid,
  task_hash brunn.sha256_hex NOT NULL,
  task_size_bytes integer NOT NULL CHECK (task_size_bytes >= 0),
  capability_snapshot text[] NOT NULL,
  authorization_scope_refs text[] NOT NULL,
  policy_projection_hash brunn.sha256_hex NOT NULL,
  mode text NOT NULL CHECK (mode IN ('read_only', 'read_write')),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  expires_at timestamptz NOT NULL,
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES brunn.api_credentials(user_id, id),
  FOREIGN KEY (user_id, corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, parent_session_id)
    REFERENCES brunn.sessions(user_id, id),
  CHECK (expires_at > created_at),
  CHECK (mode <> 'read_only' OR NOT (
    capability_snapshot && ARRAY['checkpoint', 'save', 'stage', 'correct', 'delete', 'dream']::text[]
  ))
);

CREATE TABLE brunn.session_root_refs (
  user_id uuid NOT NULL,
  session_id uuid NOT NULL,
  position integer NOT NULL CHECK (position >= 0),
  record_id uuid NOT NULL,
  PRIMARY KEY (user_id, session_id, position),
  FOREIGN KEY (user_id, session_id)
    REFERENCES brunn.sessions(user_id, id),
  FOREIGN KEY (user_id, record_id)
    REFERENCES brunn.record_keys(user_id, record_id),
  UNIQUE (user_id, session_id, record_id)
);

CREATE TABLE brunn.checkpoints (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  parent_checkpoint_id uuid,
  corpus_revision_id uuid NOT NULL,
  session_id uuid NOT NULL,
  title text,
  summary text NOT NULL DEFAULT '',
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, id, corpus_revision_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, parent_checkpoint_id)
    REFERENCES brunn.checkpoints(user_id, id),
  FOREIGN KEY (user_id, corpus_revision_id)
    REFERENCES brunn.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, session_id)
    REFERENCES brunn.sessions(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES brunn.policy_revisions(user_id, policy_id, version)
);

CREATE TRIGGER checkpoints_register_record
BEFORE INSERT ON brunn.checkpoints
FOR EACH ROW EXECUTE FUNCTION brunn.register_record_key('checkpoint');

CREATE TABLE brunn.checkpoint_focus_links (
  user_id uuid NOT NULL,
  checkpoint_id uuid NOT NULL,
  position integer NOT NULL CHECK (position >= 0),
  target_record_id uuid NOT NULL,
  lens text NOT NULL CHECK (lens IN (
    'domain', 'situation', 'activity', 'artifact', 'evidence'
  )),
  PRIMARY KEY (user_id, checkpoint_id, position),
  FOREIGN KEY (user_id, checkpoint_id)
    REFERENCES brunn.checkpoints(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES brunn.record_keys(user_id, record_id),
  UNIQUE (user_id, checkpoint_id, target_record_id, lens)
);

CREATE TABLE brunn.checkpoint_goals (
  user_id uuid NOT NULL,
  checkpoint_id uuid NOT NULL,
  position integer NOT NULL CHECK (position >= 0),
  goal_text brunn.nonempty_text NOT NULL,
  work_item_object_id uuid,
  state_claim_id uuid,
  PRIMARY KEY (user_id, checkpoint_id, position),
  FOREIGN KEY (user_id, checkpoint_id)
    REFERENCES brunn.checkpoints(user_id, id),
  FOREIGN KEY (user_id, work_item_object_id)
    REFERENCES brunn.objects(user_id, id),
  FOREIGN KEY (user_id, state_claim_id)
    REFERENCES brunn.claims(user_id, id)
);

CREATE TABLE brunn.checkpoint_state_refs (
  user_id uuid NOT NULL,
  checkpoint_id uuid NOT NULL,
  position integer NOT NULL CHECK (position >= 0),
  state_claim_id uuid NOT NULL,
  PRIMARY KEY (user_id, checkpoint_id, position),
  FOREIGN KEY (user_id, checkpoint_id)
    REFERENCES brunn.checkpoints(user_id, id),
  FOREIGN KEY (user_id, state_claim_id)
    REFERENCES brunn.state_assignments(user_id, claim_id),
  UNIQUE (user_id, checkpoint_id, state_claim_id)
);

CREATE TABLE brunn.checkpoint_gaps (
  user_id uuid NOT NULL,
  checkpoint_id uuid NOT NULL,
  position integer NOT NULL CHECK (position >= 0),
  gap_kind brunn.nonempty_text NOT NULL,
  description brunn.nonempty_text NOT NULL,
  related_record_id uuid,
  PRIMARY KEY (user_id, checkpoint_id, position),
  FOREIGN KEY (user_id, checkpoint_id)
    REFERENCES brunn.checkpoints(user_id, id),
  FOREIGN KEY (user_id, related_record_id)
    REFERENCES brunn.record_keys(user_id, record_id)
);

CREATE TABLE brunn.checkpoint_decisions (
  user_id uuid NOT NULL,
  checkpoint_id uuid NOT NULL,
  position integer NOT NULL CHECK (position >= 0),
  decision_kind text NOT NULL CHECK (decision_kind IN (
    'decision', 'hypothesis', 'rejected_path', 'constraint'
  )),
  claim_id uuid NOT NULL,
  PRIMARY KEY (user_id, checkpoint_id, position),
  FOREIGN KEY (user_id, checkpoint_id)
    REFERENCES brunn.checkpoints(user_id, id),
  FOREIGN KEY (user_id, claim_id)
    REFERENCES brunn.claims(user_id, id)
);

CREATE TABLE brunn.checkpoint_gates (
  user_id uuid NOT NULL,
  checkpoint_id uuid NOT NULL,
  position integer NOT NULL CHECK (position >= 0),
  gate_object_id uuid,
  description brunn.nonempty_text NOT NULL,
  state_claim_id uuid,
  PRIMARY KEY (user_id, checkpoint_id, position),
  FOREIGN KEY (user_id, checkpoint_id)
    REFERENCES brunn.checkpoints(user_id, id),
  FOREIGN KEY (user_id, gate_object_id)
    REFERENCES brunn.objects(user_id, id),
  FOREIGN KEY (user_id, state_claim_id)
    REFERENCES brunn.state_assignments(user_id, claim_id)
);

CREATE TABLE brunn.checkpoint_gate_evidence (
  user_id uuid NOT NULL,
  checkpoint_id uuid NOT NULL,
  gate_position integer NOT NULL,
  evidence_id uuid NOT NULL,
  PRIMARY KEY (user_id, checkpoint_id, gate_position, evidence_id),
  FOREIGN KEY (user_id, checkpoint_id, gate_position)
    REFERENCES brunn.checkpoint_gates(user_id, checkpoint_id, position),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES brunn.evidence_items(user_id, id)
);

CREATE TABLE brunn.checkpoint_actions (
  user_id uuid NOT NULL,
  checkpoint_id uuid NOT NULL,
  position integer NOT NULL CHECK (position >= 0),
  action_text brunn.nonempty_text NOT NULL,
  responsible_record_id uuid,
  due_temporal_id uuid,
  preconditions jsonb NOT NULL DEFAULT '[]'::jsonb,
  PRIMARY KEY (user_id, checkpoint_id, position),
  FOREIGN KEY (user_id, checkpoint_id)
    REFERENCES brunn.checkpoints(user_id, id),
  FOREIGN KEY (user_id, responsible_record_id)
    REFERENCES brunn.record_keys(user_id, record_id),
  FOREIGN KEY (user_id, due_temporal_id)
    REFERENCES brunn.temporal_specs(user_id, id)
);

CREATE TABLE brunn.checkpoint_source_refs (
  user_id uuid NOT NULL,
  checkpoint_id uuid NOT NULL,
  corpus_revision_id uuid NOT NULL,
  position integer NOT NULL CHECK (position >= 0),
  source_record_id uuid NOT NULL,
  source_version integer,
  locator jsonb NOT NULL DEFAULT '{}'::jsonb,
  PRIMARY KEY (user_id, checkpoint_id, position),
  FOREIGN KEY (user_id, checkpoint_id, corpus_revision_id)
    REFERENCES brunn.checkpoints(user_id, id, corpus_revision_id),
  FOREIGN KEY (user_id, corpus_revision_id, source_record_id)
    REFERENCES brunn.corpus_members(user_id, corpus_revision_id, record_id),
  CHECK (source_version IS NULL OR source_version > 0),
  UNIQUE (user_id, checkpoint_id, source_record_id, source_version)
);

ALTER TABLE brunn.sessions
  ADD COLUMN resumed_checkpoint_id uuid;

ALTER TABLE brunn.sessions
  ADD CONSTRAINT sessions_resumed_checkpoint_fk
  FOREIGN KEY (user_id, resumed_checkpoint_id)
  REFERENCES brunn.checkpoints(user_id, id)
  DEFERRABLE INITIALLY DEFERRED;

CREATE FUNCTION brunn.validate_checkpoint_resume_parent()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  resumed_parent_id uuid;
  session_scope_id uuid;
  parent_scope_id uuid;
BEGIN
  SELECT session_row.resumed_checkpoint_id, session_row.scope_id
  INTO STRICT resumed_parent_id, session_scope_id
  FROM brunn.sessions AS session_row
  WHERE session_row.user_id = NEW.user_id
    AND session_row.id = NEW.session_id;

  IF NEW.scope_id <> session_scope_id THEN
    RAISE EXCEPTION 'checkpoint and session scopes must match'
      USING ERRCODE = '23514';
  END IF;

  IF NEW.parent_checkpoint_id IS DISTINCT FROM resumed_parent_id THEN
    RAISE EXCEPTION 'checkpoint parent must exactly match the session resume link'
      USING ERRCODE = '23514';
  END IF;

  IF resumed_parent_id IS NOT NULL THEN
    SELECT parent.scope_id INTO STRICT parent_scope_id
    FROM brunn.checkpoints AS parent
    WHERE parent.user_id = NEW.user_id AND parent.id = resumed_parent_id;

    IF parent_scope_id <> NEW.scope_id THEN
      RAISE EXCEPTION 'resumed checkpoint must belong to the session scope'
        USING ERRCODE = '23514';
    END IF;
  END IF;
  RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER checkpoint_resume_parent_contract
AFTER INSERT ON brunn.checkpoints
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION brunn.validate_checkpoint_resume_parent();

CREATE FUNCTION brunn.validate_session_resume_scope()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NEW.resumed_checkpoint_id IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM brunn.checkpoints AS checkpoint
    WHERE checkpoint.user_id = NEW.user_id
      AND checkpoint.id = NEW.resumed_checkpoint_id
      AND checkpoint.scope_id = NEW.scope_id
  ) THEN
    RAISE EXCEPTION 'resumed checkpoint must belong to the session scope'
      USING ERRCODE = '23514';
  END IF;
  RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER session_resume_scope_contract
AFTER INSERT ON brunn.sessions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION brunn.validate_session_resume_scope();

CREATE FUNCTION brunn.validate_checkpoint_sources()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM brunn.checkpoint_source_refs AS source_ref
    WHERE source_ref.user_id = NEW.user_id
      AND source_ref.checkpoint_id = NEW.id
  ) THEN
    RAISE EXCEPTION 'checkpoint % requires at least one source reference', NEW.id
      USING ERRCODE = '23514';
  END IF;
  RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER checkpoint_requires_sources
AFTER INSERT ON brunn.checkpoints
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION brunn.validate_checkpoint_sources();

ALTER TABLE brunn.projection_receipts
  ADD CONSTRAINT projection_receipts_corpus_revision_fk
  FOREIGN KEY (user_id, source_corpus_revision_id)
  REFERENCES brunn.corpus_revisions(user_id, id);

CREATE INDEX checkpoint_parent_idx
  ON brunn.checkpoints (user_id, parent_checkpoint_id, created_at DESC);

CREATE INDEX sessions_revision_idx
  ON brunn.sessions (user_id, scope_id, corpus_revision_id, created_at DESC);

CREATE TRIGGER corpus_revisions_immutable
BEFORE UPDATE OR DELETE ON brunn.corpus_revisions
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER corpus_members_immutable
BEFORE UPDATE OR DELETE ON brunn.corpus_members
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER sessions_immutable
BEFORE UPDATE OR DELETE ON brunn.sessions
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER checkpoints_immutable
BEFORE UPDATE OR DELETE ON brunn.checkpoints
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

DO $$
DECLARE
  table_name text;
BEGIN
  FOREACH table_name IN ARRAY ARRAY[
    'session_root_refs', 'checkpoint_focus_links', 'checkpoint_goals',
    'checkpoint_state_refs', 'checkpoint_gaps', 'checkpoint_decisions',
    'checkpoint_gates', 'checkpoint_gate_evidence', 'checkpoint_actions',
    'checkpoint_source_refs'
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

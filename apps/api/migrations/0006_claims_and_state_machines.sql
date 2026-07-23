CREATE TABLE straylight.claims (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  predicate straylight.namespaced_identifier NOT NULL,
  value jsonb NOT NULL,
  producer_ref uuid NOT NULL,
  formation_method text NOT NULL CHECK (formation_method IN (
    'asserted', 'extracted', 'observed', 'inferred', 'synthesized', 'action_result'
  )),
  claim_mode text NOT NULL CHECK (claim_mode IN (
    'descriptive', 'preference', 'instruction', 'assessment', 'hypothesis',
    'proposal', 'state_assignment', 'state_transition'
  )),
  support_state text NOT NULL CHECK (support_state IN (
    'reported', 'supported', 'verified', 'disputed', 'refuted', 'unknown'
  )),
  authority straylight.nonempty_text NOT NULL,
  canonicality straylight.nonempty_text NOT NULL,
  confidence double precision CHECK (confidence IS NULL OR confidence BETWEEN 0 AND 1),
  valid_temporal_id uuid,
  observed_temporal_id uuid,
  source_episode_id uuid NOT NULL,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, producer_ref)
    REFERENCES straylight.record_keys(user_id, record_id),
  FOREIGN KEY (user_id, valid_temporal_id)
    REFERENCES straylight.temporal_specs(user_id, id),
  FOREIGN KEY (user_id, observed_temporal_id)
    REFERENCES straylight.temporal_specs(user_id, id),
  FOREIGN KEY (user_id, source_episode_id)
    REFERENCES straylight.source_episodes(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version)
);

CREATE TRIGGER claims_register_record
BEFORE INSERT ON straylight.claims
FOR EACH ROW EXECUTE FUNCTION straylight.register_record_key('claim');

CREATE TABLE straylight.claim_about_targets (
  user_id uuid NOT NULL,
  claim_id uuid NOT NULL,
  target_record_id uuid NOT NULL,
  role straylight.nonempty_text NOT NULL DEFAULT 'about',
  PRIMARY KEY (user_id, claim_id, target_record_id, role),
  FOREIGN KEY (user_id, claim_id)
    REFERENCES straylight.claims(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id)
);

CREATE TABLE straylight.claim_evidence_links (
  user_id uuid NOT NULL,
  claim_id uuid NOT NULL,
  evidence_id uuid NOT NULL,
  support_kind text NOT NULL CHECK (support_kind IN ('supporting', 'conflicting')),
  PRIMARY KEY (user_id, claim_id, evidence_id, support_kind),
  FOREIGN KEY (user_id, claim_id)
    REFERENCES straylight.claims(user_id, id),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES straylight.evidence_items(user_id, id)
);

CREATE TABLE straylight.claim_lineage (
  user_id uuid NOT NULL,
  claim_id uuid NOT NULL,
  predecessor_claim_id uuid NOT NULL,
  lineage_kind text NOT NULL CHECK (lineage_kind IN (
    'corrects', 'derives_from', 'contradicts', 'supersedes', 'retracts'
  )),
  evidence_id uuid,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, claim_id, predecessor_claim_id, lineage_kind),
  FOREIGN KEY (user_id, claim_id)
    REFERENCES straylight.claims(user_id, id),
  FOREIGN KEY (user_id, predecessor_claim_id)
    REFERENCES straylight.claims(user_id, id),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES straylight.evidence_items(user_id, id),
  CHECK (claim_id <> predecessor_claim_id)
);

CREATE FUNCTION straylight.validate_claim_completeness()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM straylight.claim_about_targets AS target
    WHERE target.user_id = NEW.user_id AND target.claim_id = NEW.id
  ) THEN
    RAISE EXCEPTION 'claim % must have at least one about target', NEW.id
      USING ERRCODE = '23514';
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM straylight.claim_evidence_links AS evidence_link
    WHERE evidence_link.user_id = NEW.user_id
      AND evidence_link.claim_id = NEW.id
      AND evidence_link.support_kind = 'supporting'
  ) AND NOT EXISTS (
    SELECT 1 FROM straylight.claim_lineage AS lineage
    WHERE lineage.user_id = NEW.user_id AND lineage.claim_id = NEW.id
  ) THEN
    RAISE EXCEPTION 'claim % requires supporting evidence or claim lineage', NEW.id
      USING ERRCODE = '23514';
  END IF;
  RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER claims_require_about_and_provenance
AFTER INSERT ON straylight.claims
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION straylight.validate_claim_completeness();

CREATE TABLE straylight.state_machines (
  state_machine_ref straylight.namespaced_identifier PRIMARY KEY,
  display_name straylight.nonempty_text NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  target_kinds text[] NOT NULL DEFAULT ARRAY['object', 'relation']::text[],
  description text NOT NULL DEFAULT '',
  reserved boolean NOT NULL DEFAULT false
);

CREATE TABLE straylight.state_values (
  state_machine_ref straylight.namespaced_identifier NOT NULL,
  value straylight.nonempty_text NOT NULL,
  display_order integer NOT NULL CHECK (display_order >= 0),
  terminal boolean NOT NULL DEFAULT false,
  PRIMARY KEY (state_machine_ref, value),
  FOREIGN KEY (state_machine_ref)
    REFERENCES straylight.state_machines(state_machine_ref)
);

CREATE TABLE straylight.state_transition_rules (
  state_machine_ref straylight.namespaced_identifier NOT NULL,
  from_value straylight.nonempty_text NOT NULL,
  to_value straylight.nonempty_text NOT NULL,
  rule_version integer NOT NULL DEFAULT 1 CHECK (rule_version > 0),
  conditions jsonb NOT NULL DEFAULT '{}'::jsonb,
  PRIMARY KEY (state_machine_ref, from_value, to_value, rule_version),
  FOREIGN KEY (state_machine_ref, from_value)
    REFERENCES straylight.state_values(state_machine_ref, value),
  FOREIGN KEY (state_machine_ref, to_value)
    REFERENCES straylight.state_values(state_machine_ref, value),
  CHECK (from_value <> to_value)
);

COMMENT ON TABLE straylight.state_transition_rules IS
  'Intentionally unseeded until allowed transition edges are a locked product decision.';

CREATE TABLE straylight.state_assignments (
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  claim_id uuid NOT NULL,
  target_record_id uuid NOT NULL,
  state_machine_ref straylight.namespaced_identifier NOT NULL,
  state_value straylight.nonempty_text NOT NULL,
  previous_assignment_claim_id uuid,
  PRIMARY KEY (user_id, claim_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, claim_id)
    REFERENCES straylight.claims(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id),
  FOREIGN KEY (state_machine_ref, state_value)
    REFERENCES straylight.state_values(state_machine_ref, value),
  FOREIGN KEY (user_id, previous_assignment_claim_id)
    REFERENCES straylight.state_assignments(user_id, claim_id)
    DEFERRABLE INITIALLY DEFERRED
);

CREATE FUNCTION straylight.validate_state_assignment_claim()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  source_claim straylight.claims%ROWTYPE;
BEGIN
  SELECT * INTO STRICT source_claim
  FROM straylight.claims
  WHERE user_id = NEW.user_id AND id = NEW.claim_id;

  IF source_claim.claim_mode NOT IN ('state_assignment', 'state_transition') THEN
    RAISE EXCEPTION 'state assignment claim % has incompatible mode %',
      NEW.claim_id, source_claim.claim_mode USING ERRCODE = '23514';
  END IF;
  IF source_claim.predicate::text <> NEW.state_machine_ref::text THEN
    RAISE EXCEPTION 'state assignment predicate must equal its state machine'
      USING ERRCODE = '23514';
  END IF;
  IF source_claim.value <> to_jsonb(NEW.state_value::text) THEN
    RAISE EXCEPTION 'state assignment value does not match claim value'
      USING ERRCODE = '23514';
  END IF;
  IF NOT EXISTS (
    SELECT 1 FROM straylight.claim_about_targets
    WHERE user_id = NEW.user_id
      AND claim_id = NEW.claim_id
      AND target_record_id = NEW.target_record_id
  ) THEN
    RAISE EXCEPTION 'state assignment target must be an about target of its claim'
      USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER state_assignments_validate_claim
BEFORE INSERT ON straylight.state_assignments
FOR EACH ROW EXECUTE FUNCTION straylight.validate_state_assignment_claim();

CREATE TABLE straylight.state_heads (
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  target_record_id uuid NOT NULL,
  state_machine_ref straylight.namespaced_identifier NOT NULL,
  current_claim_id uuid NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, target_record_id, state_machine_ref),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id),
  FOREIGN KEY (state_machine_ref)
    REFERENCES straylight.state_machines(state_machine_ref),
  FOREIGN KEY (user_id, current_claim_id)
    REFERENCES straylight.state_assignments(user_id, claim_id)
);

CREATE INDEX claims_current_resolution_idx
  ON straylight.claims (
    user_id, scope_id, predicate, canonicality, recorded_at DESC
  );

CREATE INDEX claim_targets_lookup_idx
  ON straylight.claim_about_targets (user_id, target_record_id, claim_id);

CREATE INDEX state_heads_machine_idx
  ON straylight.state_heads (user_id, scope_id, state_machine_ref, target_record_id);

CREATE TRIGGER claims_immutable
BEFORE UPDATE OR DELETE ON straylight.claims
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER claim_about_targets_immutable
BEFORE UPDATE OR DELETE ON straylight.claim_about_targets
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER claim_evidence_links_immutable
BEFORE UPDATE OR DELETE ON straylight.claim_evidence_links
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER claim_lineage_immutable
BEFORE UPDATE OR DELETE ON straylight.claim_lineage
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER state_assignments_immutable
BEFORE UPDATE OR DELETE ON straylight.state_assignments
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER state_heads_no_delete
BEFORE DELETE ON straylight.state_heads
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_physical_delete();

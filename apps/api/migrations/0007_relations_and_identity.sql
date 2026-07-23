CREATE TABLE straylight.relation_schema_revisions (
  predicate straylight.namespaced_identifier NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  display_name straylight.nonempty_text NOT NULL,
  qualifier_schema jsonb NOT NULL DEFAULT '{}'::jsonb,
  allowed_state_machines text[] NOT NULL DEFAULT '{}'::text[],
  allow_extra_roles boolean NOT NULL DEFAULT false,
  inverse_label text,
  reserved boolean NOT NULL DEFAULT false,
  PRIMARY KEY (predicate, version)
);

CREATE TABLE straylight.relation_role_rules (
  predicate straylight.namespaced_identifier NOT NULL,
  schema_version integer NOT NULL,
  role straylight.nonempty_text NOT NULL,
  minimum_count integer NOT NULL DEFAULT 0 CHECK (minimum_count >= 0),
  maximum_count integer CHECK (maximum_count IS NULL OR maximum_count >= minimum_count),
  allowed_profiles text[] NOT NULL DEFAULT '{}'::text[],
  PRIMARY KEY (predicate, schema_version, role),
  FOREIGN KEY (predicate, schema_version)
    REFERENCES straylight.relation_schema_revisions(predicate, version)
);

CREATE TABLE straylight.relations (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  current_version integer NOT NULL DEFAULT 1 CHECK (current_version > 0),
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version)
);

CREATE TRIGGER relations_register_record
BEFORE INSERT ON straylight.relations
FOR EACH ROW EXECUTE FUNCTION straylight.register_record_key('relation');

CREATE TABLE straylight.relation_revisions (
  user_id uuid NOT NULL,
  relation_id uuid NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  previous_version integer,
  predicate straylight.namespaced_identifier NOT NULL,
  schema_version integer NOT NULL,
  qualifiers jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(qualifiers) = 'object'),
  valid_temporal_id uuid,
  source_episode_id uuid NOT NULL,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, relation_id, version),
  FOREIGN KEY (user_id, relation_id)
    REFERENCES straylight.relations(user_id, id),
  FOREIGN KEY (user_id, relation_id, previous_version)
    REFERENCES straylight.relation_revisions(user_id, relation_id, version)
    DEFERRABLE INITIALLY DEFERRED,
  FOREIGN KEY (predicate, schema_version)
    REFERENCES straylight.relation_schema_revisions(predicate, version),
  FOREIGN KEY (user_id, valid_temporal_id)
    REFERENCES straylight.temporal_specs(user_id, id),
  FOREIGN KEY (user_id, source_episode_id)
    REFERENCES straylight.source_episodes(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version),
  CHECK (
    (version = 1 AND previous_version IS NULL)
    OR (version > 1 AND previous_version = version - 1)
  )
);

ALTER TABLE straylight.relations
  ADD CONSTRAINT relations_current_revision_fk
  FOREIGN KEY (user_id, id, current_version)
  REFERENCES straylight.relation_revisions(user_id, relation_id, version)
  DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE straylight.relation_endpoints (
  user_id uuid NOT NULL,
  relation_id uuid NOT NULL,
  relation_version integer NOT NULL,
  role straylight.nonempty_text NOT NULL,
  position integer NOT NULL CHECK (position >= 0),
  target_record_id uuid NOT NULL,
  PRIMARY KEY (user_id, relation_id, relation_version, role, position),
  FOREIGN KEY (user_id, relation_id, relation_version)
    REFERENCES straylight.relation_revisions(user_id, relation_id, version),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id),
  UNIQUE (user_id, relation_id, relation_version, role, target_record_id)
);

CREATE TABLE straylight.relation_evidence_links (
  user_id uuid NOT NULL,
  relation_id uuid NOT NULL,
  relation_version integer NOT NULL,
  evidence_id uuid NOT NULL,
  support_kind text NOT NULL CHECK (support_kind IN ('supporting', 'conflicting')),
  PRIMARY KEY (user_id, relation_id, relation_version, evidence_id, support_kind),
  FOREIGN KEY (user_id, relation_id, relation_version)
    REFERENCES straylight.relation_revisions(user_id, relation_id, version),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES straylight.evidence_items(user_id, id)
);

CREATE FUNCTION straylight.validate_relation_revision()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  relation_schema straylight.relation_schema_revisions%ROWTYPE;
  rule straylight.relation_role_rules%ROWTYPE;
  role_count integer;
BEGIN
  SELECT * INTO STRICT relation_schema
  FROM straylight.relation_schema_revisions
  WHERE predicate = NEW.predicate AND version = NEW.schema_version;

  IF NOT EXISTS (
    SELECT 1 FROM straylight.relation_endpoints AS endpoint
    WHERE endpoint.user_id = NEW.user_id
      AND endpoint.relation_id = NEW.relation_id
      AND endpoint.relation_version = NEW.version
  ) THEN
    RAISE EXCEPTION 'relation revision %/% requires endpoints',
      NEW.relation_id, NEW.version USING ERRCODE = '23514';
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM straylight.relation_evidence_links AS evidence_link
    WHERE evidence_link.user_id = NEW.user_id
      AND evidence_link.relation_id = NEW.relation_id
      AND evidence_link.relation_version = NEW.version
      AND evidence_link.support_kind = 'supporting'
  ) THEN
    RAISE EXCEPTION 'relation revision %/% requires supporting evidence',
      NEW.relation_id, NEW.version USING ERRCODE = '23514';
  END IF;

  FOR rule IN
    SELECT * FROM straylight.relation_role_rules
    WHERE predicate = NEW.predicate AND schema_version = NEW.schema_version
  LOOP
    SELECT count(*) INTO role_count
    FROM straylight.relation_endpoints
    WHERE user_id = NEW.user_id
      AND relation_id = NEW.relation_id
      AND relation_version = NEW.version
      AND role = rule.role;

    IF role_count < rule.minimum_count
       OR (rule.maximum_count IS NOT NULL AND role_count > rule.maximum_count) THEN
      RAISE EXCEPTION 'relation role % has invalid cardinality %', rule.role, role_count
        USING ERRCODE = '23514';
    END IF;

    IF cardinality(rule.allowed_profiles) > 0 AND EXISTS (
      SELECT 1
      FROM straylight.relation_endpoints AS endpoint
      JOIN straylight.record_keys AS record_key
        ON record_key.user_id = endpoint.user_id
       AND record_key.record_id = endpoint.target_record_id
      WHERE endpoint.user_id = NEW.user_id
        AND endpoint.relation_id = NEW.relation_id
        AND endpoint.relation_version = NEW.version
        AND endpoint.role = rule.role
        AND (
          record_key.record_kind <> 'object'
          OR NOT EXISTS (
            SELECT 1
            FROM straylight.objects AS object_row
            JOIN straylight.object_revision_profiles AS profile
              ON profile.user_id = object_row.user_id
             AND profile.object_id = object_row.id
             AND profile.object_version = object_row.current_version
            WHERE object_row.user_id = endpoint.user_id
              AND object_row.id = endpoint.target_record_id
              AND profile.profile_ref::text = ANY(rule.allowed_profiles)
          )
        )
    ) THEN
      RAISE EXCEPTION 'relation endpoint violates allowed profiles for role %', rule.role
        USING ERRCODE = '23514';
    END IF;
  END LOOP;

  IF NOT relation_schema.allow_extra_roles AND EXISTS (
    SELECT 1
    FROM straylight.relation_endpoints AS endpoint
    WHERE endpoint.user_id = NEW.user_id
      AND endpoint.relation_id = NEW.relation_id
      AND endpoint.relation_version = NEW.version
      AND NOT EXISTS (
        SELECT 1 FROM straylight.relation_role_rules AS allowed_role
        WHERE allowed_role.predicate = NEW.predicate
          AND allowed_role.schema_version = NEW.schema_version
          AND allowed_role.role = endpoint.role
      )
  ) THEN
    RAISE EXCEPTION 'relation revision contains an undeclared endpoint role'
      USING ERRCODE = '23514';
  END IF;

  RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER relation_revision_contract
AFTER INSERT ON straylight.relation_revisions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION straylight.validate_relation_revision();

CREATE TABLE straylight.identity_name_claims (
  user_id uuid NOT NULL,
  claim_id uuid NOT NULL,
  object_id uuid NOT NULL,
  name_kind straylight.nonempty_text NOT NULL,
  name_value straylight.nonempty_text NOT NULL,
  locale text,
  valid_temporal_id uuid,
  PRIMARY KEY (user_id, claim_id),
  FOREIGN KEY (user_id, claim_id)
    REFERENCES straylight.claims(user_id, id),
  FOREIGN KEY (user_id, object_id)
    REFERENCES straylight.objects(user_id, id),
  FOREIGN KEY (user_id, valid_temporal_id)
    REFERENCES straylight.temporal_specs(user_id, id)
);

CREATE TABLE straylight.external_identifiers (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  object_id uuid NOT NULL,
  namespace straylight.nonempty_text NOT NULL,
  identifier_value straylight.nonempty_text NOT NULL,
  issuer text,
  source_episode_id uuid NOT NULL,
  evidence_id uuid NOT NULL,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, object_id)
    REFERENCES straylight.objects(user_id, id),
  FOREIGN KEY (user_id, source_episode_id)
    REFERENCES straylight.source_episodes(user_id, id),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES straylight.evidence_items(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version)
);

CREATE INDEX external_identifiers_lookup_idx
  ON straylight.external_identifiers (user_id, scope_id, namespace, identifier_value);

CREATE TABLE straylight.identity_review_receipts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  relation_id uuid NOT NULL,
  decision text NOT NULL CHECK (decision IN ('approved', 'rejected', 'reversed')),
  reviewer_ref uuid NOT NULL,
  reason straylight.nonempty_text NOT NULL,
  evidence_id uuid NOT NULL,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, relation_id)
    REFERENCES straylight.relations(user_id, id),
  FOREIGN KEY (user_id, reviewer_ref)
    REFERENCES straylight.record_keys(user_id, record_id),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES straylight.evidence_items(user_id, id)
);

CREATE FUNCTION straylight.require_reviewed_identity_equivalence()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
  requested_status text := coalesce(NEW.qualifiers ->> 'equivalence_status', 'active');
  required_decision text;
BEGIN
  IF NEW.predicate::text <> 'core.same_as' THEN
    RETURN NEW;
  END IF;

  required_decision := CASE requested_status
    WHEN 'reversed' THEN 'reversed'
    ELSE 'approved'
  END;

  IF NOT EXISTS (
    SELECT 1 FROM straylight.identity_review_receipts AS receipt
    WHERE receipt.user_id = NEW.user_id
      AND receipt.relation_id = NEW.relation_id
      AND receipt.decision = required_decision
  ) THEN
    RAISE EXCEPTION 'core.same_as requires a reviewed identity decision (%)',
      required_decision USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER relation_revisions_require_identity_review
BEFORE INSERT ON straylight.relation_revisions
FOR EACH ROW EXECUTE FUNCTION straylight.require_reviewed_identity_equivalence();

CREATE TRIGGER relation_revisions_immutable
BEFORE UPDATE OR DELETE ON straylight.relation_revisions
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER relation_endpoints_immutable
BEFORE UPDATE OR DELETE ON straylight.relation_endpoints
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER relation_evidence_links_immutable
BEFORE UPDATE OR DELETE ON straylight.relation_evidence_links
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER identity_name_claims_immutable
BEFORE UPDATE OR DELETE ON straylight.identity_name_claims
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER external_identifiers_immutable
BEFORE UPDATE OR DELETE ON straylight.external_identifiers
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER identity_review_receipts_immutable
BEFORE UPDATE OR DELETE ON straylight.identity_review_receipts
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER relations_head_advance
BEFORE UPDATE OF current_version ON straylight.relations
FOR EACH ROW EXECUTE FUNCTION straylight.enforce_sequential_head_advance();

CREATE TRIGGER relations_no_delete
BEFORE DELETE ON straylight.relations
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_physical_delete();


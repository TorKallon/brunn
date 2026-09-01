CREATE TABLE brunn.profile_schema_revisions (
  profile_ref brunn.namespaced_identifier NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  display_name brunn.nonempty_text NOT NULL,
  properties_schema jsonb NOT NULL DEFAULT '{}'::jsonb,
  reserved boolean NOT NULL DEFAULT false,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (profile_ref, version)
);

CREATE TABLE brunn.objects (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  current_version integer NOT NULL DEFAULT 1 CHECK (current_version > 0),
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES brunn.policy_revisions(user_id, policy_id, version)
);

CREATE TRIGGER objects_register_record
BEFORE INSERT ON brunn.objects
FOR EACH ROW EXECUTE FUNCTION brunn.register_record_key('object');

CREATE TABLE brunn.object_revisions (
  user_id uuid NOT NULL,
  object_id uuid NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  previous_version integer,
  label text,
  properties jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(properties) = 'object'),
  source_episode_id uuid NOT NULL,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, object_id, version),
  FOREIGN KEY (user_id, object_id)
    REFERENCES brunn.objects(user_id, id),
  FOREIGN KEY (user_id, object_id, previous_version)
    REFERENCES brunn.object_revisions(user_id, object_id, version)
    DEFERRABLE INITIALLY DEFERRED,
  FOREIGN KEY (user_id, source_episode_id)
    REFERENCES brunn.source_episodes(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES brunn.policy_revisions(user_id, policy_id, version),
  CHECK (
    (version = 1 AND previous_version IS NULL)
    OR (version > 1 AND previous_version = version - 1)
  )
);

ALTER TABLE brunn.objects
  ADD CONSTRAINT objects_current_revision_fk
  FOREIGN KEY (user_id, id, current_version)
  REFERENCES brunn.object_revisions(user_id, object_id, version)
  DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE brunn.object_revision_profiles (
  user_id uuid NOT NULL,
  object_id uuid NOT NULL,
  object_version integer NOT NULL,
  profile_ref brunn.namespaced_identifier NOT NULL,
  profile_version integer NOT NULL,
  PRIMARY KEY (user_id, object_id, object_version, profile_ref),
  FOREIGN KEY (user_id, object_id, object_version)
    REFERENCES brunn.object_revisions(user_id, object_id, version),
  FOREIGN KEY (profile_ref, profile_version)
    REFERENCES brunn.profile_schema_revisions(profile_ref, version)
);

CREATE FUNCTION brunn.validate_object_revision_profiles()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NOT EXISTS (
    SELECT 1
    FROM brunn.object_revision_profiles AS profile
    WHERE profile.user_id = NEW.user_id
      AND profile.object_id = NEW.object_id
      AND profile.object_version = NEW.version
  ) THEN
    RAISE EXCEPTION 'object revision %/% must have at least one type profile',
      NEW.object_id, NEW.version
      USING ERRCODE = '23514';
  END IF;
  RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER object_revision_requires_profile
AFTER INSERT ON brunn.object_revisions
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION brunn.validate_object_revision_profiles();

CREATE TABLE brunn.object_handles (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  handle_kind brunn.nonempty_text NOT NULL,
  handle brunn.nonempty_text NOT NULL,
  object_id uuid NOT NULL,
  valid_from timestamptz NOT NULL DEFAULT clock_timestamp(),
  valid_to timestamptz,
  evidence_id uuid NOT NULL,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES brunn.scopes(user_id, id),
  FOREIGN KEY (user_id, object_id)
    REFERENCES brunn.objects(user_id, id),
  FOREIGN KEY (user_id, evidence_id)
    REFERENCES brunn.evidence_items(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES brunn.policy_revisions(user_id, policy_id, version),
  CHECK (valid_to IS NULL OR valid_to > valid_from)
);

CREATE UNIQUE INDEX object_handles_current_idx
  ON brunn.object_handles (user_id, scope_id, handle_kind, handle)
  WHERE valid_to IS NULL;

CREATE TABLE brunn.artifact_asset_links (
  user_id uuid NOT NULL,
  object_id uuid NOT NULL,
  object_version integer NOT NULL,
  asset_id uuid NOT NULL,
  asset_version integer NOT NULL,
  role brunn.nonempty_text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (
    user_id, object_id, object_version, asset_id, asset_version, role
  ),
  FOREIGN KEY (user_id, object_id, object_version)
    REFERENCES brunn.object_revisions(user_id, object_id, version),
  FOREIGN KEY (user_id, asset_id, asset_version)
    REFERENCES brunn.asset_versions(user_id, asset_id, version)
);

CREATE INDEX object_profiles_lookup_idx
  ON brunn.object_revision_profiles (profile_ref, user_id, object_id, object_version);

CREATE INDEX object_revisions_recorded_idx
  ON brunn.object_revisions (user_id, object_id, recorded_at DESC);

CREATE TRIGGER object_revisions_immutable
BEFORE UPDATE OR DELETE ON brunn.object_revisions
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER object_revision_profiles_immutable
BEFORE UPDATE OR DELETE ON brunn.object_revision_profiles
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER artifact_asset_links_immutable
BEFORE UPDATE OR DELETE ON brunn.artifact_asset_links
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

CREATE TRIGGER objects_head_advance
BEFORE UPDATE OF current_version ON brunn.objects
FOR EACH ROW EXECUTE FUNCTION brunn.enforce_sequential_head_advance();

CREATE TRIGGER objects_no_delete
BEFORE DELETE ON brunn.objects
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_physical_delete();


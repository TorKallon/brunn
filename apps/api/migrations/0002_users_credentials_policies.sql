CREATE TABLE straylight.users (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  external_ref straylight.nonempty_text NOT NULL UNIQUE,
  display_name straylight.nonempty_text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE straylight.scopes (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES straylight.users(id),
  scope_ref straylight.nonempty_text NOT NULL,
  name straylight.nonempty_text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, scope_ref),
  CHECK (scope_ref !~ ',')
);

CREATE TABLE straylight.api_credentials (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES straylight.users(id),
  label straylight.nonempty_text NOT NULL,
  token_hash straylight.nonempty_text NOT NULL UNIQUE,
  capabilities text[] NOT NULL,
  disabled_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  CHECK (cardinality(capabilities) > 0),
  CHECK (array_position(capabilities, '') IS NULL),
  CHECK (capabilities <@ ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream'
  ]::text[])
);

CREATE TABLE straylight.credential_scope_grants (
  credential_id uuid NOT NULL,
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  PRIMARY KEY (credential_id, user_id, scope_id),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id)
);

CREATE INDEX credential_scope_grants_scope_idx
  ON straylight.credential_scope_grants (user_id, scope_id, credential_id);

CREATE TABLE straylight.policies (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES straylight.users(id),
  policy_ref straylight.nonempty_text NOT NULL,
  name straylight.nonempty_text NOT NULL,
  current_version integer NOT NULL DEFAULT 1 CHECK (current_version > 0),
  is_default boolean NOT NULL DEFAULT false,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, policy_ref)
);

CREATE UNIQUE INDEX policies_one_default_per_user_idx
  ON straylight.policies (user_id)
  WHERE is_default;

CREATE TABLE straylight.policy_revisions (
  user_id uuid NOT NULL,
  policy_id uuid NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  previous_version integer,
  default_effect text NOT NULL CHECK (default_effect IN ('allow', 'deny')),
  rules jsonb NOT NULL CHECK (jsonb_typeof(rules) = 'array'),
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, policy_id, version),
  FOREIGN KEY (user_id, policy_id)
    REFERENCES straylight.policies(user_id, id),
  FOREIGN KEY (user_id, policy_id, previous_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version)
    DEFERRABLE INITIALLY DEFERRED,
  CHECK (
    (version = 1 AND previous_version IS NULL)
    OR (version > 1 AND previous_version = version - 1)
  )
);

ALTER TABLE straylight.policies
  ADD CONSTRAINT policies_current_revision_fk
  FOREIGN KEY (user_id, id, current_version)
  REFERENCES straylight.policy_revisions(user_id, policy_id, version)
  DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE straylight.policy_rules (
  user_id uuid NOT NULL,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  rule_order integer NOT NULL CHECK (rule_order >= 0),
  effect text NOT NULL CHECK (effect IN ('allow', 'deny')),
  principals text[] NOT NULL,
  purposes text[] NOT NULL,
  actions text[] NOT NULL,
  paths text[] NOT NULL,
  conditions jsonb NOT NULL DEFAULT '{}'::jsonb,
  PRIMARY KEY (user_id, policy_id, policy_version, rule_order),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version)
);

CREATE TABLE straylight.record_keys (
  user_id uuid NOT NULL,
  record_id uuid NOT NULL,
  record_kind straylight.nonempty_text NOT NULL,
  scope_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, record_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id)
);

CREATE INDEX record_keys_kind_idx
  ON straylight.record_keys (user_id, scope_id, record_kind, record_id);

CREATE TABLE straylight.policy_bindings (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  target_record_id uuid NOT NULL,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  binding_level text NOT NULL CHECK (
    binding_level IN ('object', 'relation', 'claim', 'evidence', 'asset', 'derivative')
  ),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version),
  UNIQUE (user_id, target_record_id, binding_level)
);

CREATE TABLE straylight.field_policy_bindings (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  target_record_id uuid NOT NULL,
  json_pointer straylight.nonempty_text NOT NULL CHECK (json_pointer ~ '^(/([^/~]|~[01])*)*$'),
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version),
  UNIQUE (user_id, target_record_id, json_pointer)
);

CREATE TABLE straylight.redaction_transforms (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  transform_ref straylight.nonempty_text NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  input_schema jsonb NOT NULL,
  output_schema jsonb NOT NULL,
  implementation_ref straylight.nonempty_text NOT NULL,
  evidence jsonb NOT NULL DEFAULT '[]'::jsonb,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, transform_ref, version),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id)
);

CREATE TABLE straylight.projection_receipts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  source_corpus_revision_id uuid,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  audience straylight.nonempty_text NOT NULL,
  purpose straylight.nonempty_text NOT NULL,
  included_paths text[] NOT NULL DEFAULT '{}'::text[],
  withheld jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(withheld) = 'array'),
  transforms text[] NOT NULL DEFAULT '{}'::text[],
  output_hash straylight.sha256_hex,
  generated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version)
);

CREATE TABLE straylight.audit_events (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES straylight.users(id),
  scope_id uuid,
  credential_id uuid,
  actor_ref text,
  action straylight.nonempty_text NOT NULL,
  target_record_id uuid,
  request_id text,
  details jsonb NOT NULL DEFAULT '{}'::jsonb,
  content_free boolean NOT NULL DEFAULT true,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id)
);

CREATE FUNCTION straylight.register_record_key()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
AS $$
BEGIN
  INSERT INTO straylight.record_keys (user_id, record_id, record_kind, scope_id)
  VALUES (NEW.user_id, NEW.id, TG_ARGV[0], NEW.scope_id);
  RETURN NEW;
END;
$$;

CREATE FUNCTION straylight_auth.setting_uuid(p_name text)
RETURNS uuid
LANGUAGE plpgsql
STABLE
AS $$
DECLARE
  raw_value text;
BEGIN
  raw_value := nullif(current_setting(p_name, true), '');
  IF raw_value IS NULL THEN
    RETURN NULL;
  END IF;
  RETURN raw_value::uuid;
EXCEPTION WHEN invalid_text_representation THEN
  RETURN NULL;
END;
$$;

CREATE FUNCTION straylight_auth.current_user_id()
RETURNS uuid
LANGUAGE sql
STABLE
AS $$
  SELECT straylight_auth.setting_uuid('app.current_user_id')
$$;

CREATE FUNCTION straylight_auth.current_credential_id()
RETURNS uuid
LANGUAGE sql
STABLE
AS $$
  SELECT straylight_auth.setting_uuid('app.current_credential_id')
$$;

CREATE FUNCTION straylight_auth.setting_list(p_name text)
RETURNS text[]
LANGUAGE sql
STABLE
AS $$
  SELECT coalesce(array_agg(btrim(item)) FILTER (WHERE btrim(item) <> ''), '{}'::text[])
  FROM unnest(string_to_array(coalesce(current_setting(p_name, true), ''), ',')) AS item
$$;

CREATE FUNCTION straylight_auth.current_capabilities()
RETURNS text[]
LANGUAGE sql
STABLE
AS $$
  SELECT straylight_auth.setting_list('app.capabilities')
$$;

CREATE FUNCTION straylight_auth.current_scope_refs()
RETURNS text[]
LANGUAGE sql
STABLE
AS $$
  SELECT straylight_auth.setting_list('app.scope_refs')
$$;

CREATE FUNCTION straylight_auth.authenticate_credential(p_token_hash text)
RETURNS TABLE (
  credential_id uuid,
  user_id uuid,
  capabilities text[],
  scope_refs text[]
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
  SELECT
    credential.id,
    credential.user_id,
    credential.capabilities,
    coalesce(
      array_agg(scope.scope_ref::text ORDER BY scope.scope_ref)
        FILTER (WHERE scope.scope_ref IS NOT NULL),
      '{}'::text[]
    )
  FROM straylight.api_credentials AS credential
  LEFT JOIN straylight.credential_scope_grants AS grant_row
    ON grant_row.user_id = credential.user_id
   AND grant_row.credential_id = credential.id
  LEFT JOIN straylight.scopes AS scope
    ON scope.user_id = grant_row.user_id
   AND scope.id = grant_row.scope_id
  WHERE credential.token_hash = p_token_hash
    AND credential.disabled_at IS NULL
  GROUP BY credential.id, credential.user_id, credential.capabilities
$$;

CREATE FUNCTION straylight_auth.context_is_valid()
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
  SELECT EXISTS (
    SELECT 1
    FROM straylight.api_credentials AS credential
    WHERE credential.id = straylight_auth.current_credential_id()
      AND credential.user_id = straylight_auth.current_user_id()
      AND credential.disabled_at IS NULL
      AND straylight_auth.current_capabilities() <@ credential.capabilities
      AND straylight_auth.current_scope_refs() <@ coalesce(
        (
          SELECT array_agg(scope.scope_ref::text ORDER BY scope.scope_ref)
          FROM straylight.credential_scope_grants AS grant_row
          JOIN straylight.scopes AS scope
            ON scope.user_id = grant_row.user_id
           AND scope.id = grant_row.scope_id
          WHERE grant_row.user_id = credential.user_id
            AND grant_row.credential_id = credential.id
        ),
        '{}'::text[]
      )
  )
$$;

CREATE FUNCTION straylight_auth.has_capability(p_capability text)
RETURNS boolean
LANGUAGE sql
STABLE
AS $$
  SELECT straylight_auth.context_is_valid()
    AND p_capability = ANY(straylight_auth.current_capabilities())
$$;

CREATE FUNCTION straylight_auth.has_any_capability(p_capabilities text[])
RETURNS boolean
LANGUAGE sql
STABLE
AS $$
  SELECT straylight_auth.context_is_valid()
    AND straylight_auth.current_capabilities() && p_capabilities
$$;

CREATE FUNCTION straylight_auth.can_access_scope(p_user_id uuid, p_scope_id uuid)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
  SELECT straylight_auth.context_is_valid()
    AND p_user_id = straylight_auth.current_user_id()
    AND EXISTS (
      SELECT 1
      FROM straylight.credential_scope_grants AS grant_row
      JOIN straylight.scopes AS scope
        ON scope.user_id = grant_row.user_id
       AND scope.id = grant_row.scope_id
      WHERE grant_row.user_id = p_user_id
        AND grant_row.credential_id = straylight_auth.current_credential_id()
        AND grant_row.scope_id = p_scope_id
        AND scope.scope_ref = ANY(straylight_auth.current_scope_refs())
    )
$$;

CREATE FUNCTION straylight_auth.seed_user_defaults()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
DECLARE
  default_policy_id uuid := gen_random_uuid();
BEGIN
  INSERT INTO straylight.scopes (id, user_id, scope_ref, name)
  VALUES (gen_random_uuid(), NEW.id, 'scope:root', 'Private root');

  INSERT INTO straylight.policies (
    id, user_id, policy_ref, name, current_version, is_default
  ) VALUES (
    default_policy_id, NEW.id, 'policy:private-default',
    'Private by default', 1, true
  );

  INSERT INTO straylight.policy_revisions (
    user_id, policy_id, version, previous_version, default_effect, rules
  ) VALUES (
    NEW.id,
    default_policy_id,
    1,
    NULL,
    'deny',
    jsonb_build_array(jsonb_build_object(
      'effect', 'allow',
      'principals', jsonb_build_array('principal:self'),
      'purposes', jsonb_build_array('*'),
      'actions', jsonb_build_array('read', 'write', 'derive', 'export'),
      'paths', jsonb_build_array('')
    ))
  );

  INSERT INTO straylight.policy_rules (
    user_id, policy_id, policy_version, rule_order, effect,
    principals, purposes, actions, paths
  ) VALUES (
    NEW.id, default_policy_id, 1, 0, 'allow',
    ARRAY['principal:self'], ARRAY['*'],
    ARRAY['read', 'write', 'derive', 'export'], ARRAY['']
  );

  RETURN NEW;
END;
$$;

CREATE TRIGGER users_seed_defaults
AFTER INSERT ON straylight.users
FOR EACH ROW EXECUTE FUNCTION straylight_auth.seed_user_defaults();

CREATE FUNCTION straylight_auth.bootstrap_user(
  p_external_ref text,
  p_display_name text,
  p_credential_label text,
  p_token_hash text,
  p_capabilities text[] DEFAULT ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream'
  ]::text[]
)
RETURNS TABLE (
  user_id uuid,
  credential_id uuid,
  scope_id uuid,
  policy_id uuid
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
DECLARE
  created_user_id uuid;
  created_credential_id uuid;
  root_scope_id uuid;
  default_policy_id uuid;
  existing_credential_user_id uuid;
BEGIN
  IF p_capabilities IS NULL OR cardinality(p_capabilities) = 0 THEN
    RAISE EXCEPTION 'at least one capability is required' USING ERRCODE = '22023';
  END IF;

  INSERT INTO straylight.users (external_ref, display_name)
  VALUES (p_external_ref, p_display_name)
  ON CONFLICT (external_ref) DO UPDATE
    SET display_name = EXCLUDED.display_name
  RETURNING id INTO created_user_id;

  SELECT scope_row.id INTO root_scope_id
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = created_user_id
    AND scope_row.scope_ref = 'scope:root';

  SELECT policy_row.id INTO default_policy_id
  FROM straylight.policies AS policy_row
  WHERE policy_row.user_id = created_user_id
    AND policy_row.is_default;

  SELECT credential.user_id INTO existing_credential_user_id
  FROM straylight.api_credentials AS credential
  WHERE credential.token_hash = p_token_hash;

  IF existing_credential_user_id IS NOT NULL
     AND existing_credential_user_id <> created_user_id THEN
    RAISE EXCEPTION 'credential token hash already belongs to another user'
      USING ERRCODE = '23505';
  END IF;

  INSERT INTO straylight.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    created_user_id, p_credential_label, p_token_hash, p_capabilities
  )
  ON CONFLICT (token_hash) DO UPDATE
    SET label = EXCLUDED.label,
        capabilities = EXCLUDED.capabilities,
        disabled_at = NULL
  RETURNING id INTO created_credential_id;

  INSERT INTO straylight.credential_scope_grants (
    credential_id, user_id, scope_id
  ) VALUES (
    created_credential_id, created_user_id, root_scope_id
  ) ON CONFLICT DO NOTHING;

  RETURN QUERY SELECT
    created_user_id, created_credential_id, root_scope_id, default_policy_id;
END;
$$;

CREATE TRIGGER policy_revisions_immutable
BEFORE UPDATE OR DELETE ON straylight.policy_revisions
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER policy_rules_immutable
BEFORE UPDATE OR DELETE ON straylight.policy_rules
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER projection_receipts_immutable
BEFORE UPDATE OR DELETE ON straylight.projection_receipts
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER audit_events_immutable
BEFORE UPDATE OR DELETE ON straylight.audit_events
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

REVOKE ALL ON FUNCTION straylight_auth.authenticate_credential(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.bootstrap_user(text, text, text, text, text[]) FROM PUBLIC;
GRANT USAGE ON SCHEMA straylight_auth TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION straylight_auth.authenticate_credential(text) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION straylight_auth.bootstrap_user(text, text, text, text, text[]) TO app_rw;

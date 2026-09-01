-- Trusted-agent secret vault. Secret values are AES-256-GCM ciphertext bound
-- to user, secret, and version; the wrapping key lives outside the database.
-- Secrets are deliberately separate from the memory corpus: no embeddings,
-- no dreaming, no search, and no plaintext anywhere in Postgres.

ALTER TABLE brunn.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_capabilities_check2;

ALTER TABLE brunn.api_credentials
  ADD CONSTRAINT api_credentials_capabilities_check2 CHECK (
    capabilities <@ ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'notification:publish', 'notification:manage',
      'secret:read', 'secret:write',
      'admin'
    ]::text[]
  );

ALTER TABLE brunn.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_owner_full_capabilities_check;

UPDATE brunn.api_credentials
SET capabilities = capabilities
  || ARRAY['secret:read', 'secret:write']::text[]
WHERE capabilities @> ARRAY['credential:manage']::text[]
  AND NOT capabilities @> ARRAY['secret:read', 'secret:write']::text[];

ALTER TABLE brunn.api_credentials
  ADD CONSTRAINT api_credentials_owner_full_capabilities_check CHECK (
    NOT capabilities @> ARRAY['credential:manage']::text[]
    OR capabilities @> ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'notification:publish', 'notification:manage',
      'secret:read', 'secret:write',
      'admin'
    ]::text[]
  );

CREATE OR REPLACE FUNCTION brunn_auth.admin_issue_credential(
  p_user_id uuid,
  p_label text,
  p_token_hash text,
  p_capabilities text[],
  p_scope_refs text[]
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  allowed_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'notification:publish', 'notification:manage',
    'secret:read', 'secret:write',
    'admin'
  ];
  created_credential_id uuid;
  matched_scope_count integer;
BEGIN
  PERFORM brunn_auth.require_admin();
  IF p_capabilities IS NULL
     OR cardinality(p_capabilities) = 0
     OR array_position(p_capabilities, NULL) IS NOT NULL
     OR NOT p_capabilities <@ allowed_capabilities
     OR cardinality(p_capabilities) <> (
       SELECT count(DISTINCT capability)
       FROM unnest(p_capabilities) AS capability
     ) THEN
    RAISE EXCEPTION 'recovery capabilities are invalid'
      USING ERRCODE = '22023';
  END IF;
  IF p_scope_refs IS NULL
     OR cardinality(p_scope_refs) = 0
     OR array_position(p_scope_refs, NULL) IS NOT NULL
     OR cardinality(p_scope_refs) <> (
       SELECT count(DISTINCT scope_ref)
       FROM unnest(p_scope_refs) AS scope_ref
     ) THEN
    RAISE EXCEPTION 'recovery scope_refs must be a nonempty set'
      USING ERRCODE = '22023';
  END IF;
  IF NOT EXISTS (
    SELECT 1 FROM brunn.users
    WHERE id = p_user_id AND account_status = 'active'
  ) THEN
    RAISE EXCEPTION 'active recovery user not found' USING ERRCODE = 'P0002';
  END IF;
  SELECT count(*) INTO matched_scope_count
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  IF matched_scope_count <> cardinality(p_scope_refs) THEN
    RAISE EXCEPTION 'one or more recovery scopes do not belong to the user'
      USING ERRCODE = '22023';
  END IF;
  INSERT INTO brunn.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (p_user_id, p_label, p_token_hash, p_capabilities)
  RETURNING id INTO created_credential_id;
  INSERT INTO brunn.credential_scope_grants (
    credential_id, user_id, scope_id
  )
  SELECT created_credential_id, p_user_id, scope_row.id
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  INSERT INTO brunn.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    brunn_auth.current_user_id(),
    brunn_auth.current_credential_id(),
    'admin.credential.recover',
    jsonb_build_object(
      'target_user_id', p_user_id,
      'credential_id', created_credential_id,
      'scope_refs', p_scope_refs
    ),
    true
  );
  RETURN created_credential_id;
END;
$$;

CREATE OR REPLACE FUNCTION brunn_auth.issue_credential(
  p_user_id uuid,
  p_label text,
  p_token_hash text,
  p_capabilities text[],
  p_scope_refs text[]
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  allowed_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'notification:publish', 'notification:manage',
    'secret:read', 'secret:write',
    'admin'
  ];
  created_credential_id uuid;
  matched_scope_count integer;
BEGIN
  PERFORM brunn_auth.require_credential_control(p_user_id);
  IF p_capabilities IS NULL
     OR cardinality(p_capabilities) = 0
     OR array_position(p_capabilities, NULL) IS NOT NULL
     OR NOT p_capabilities <@ allowed_capabilities
     OR NOT p_capabilities <@ brunn_auth.current_capabilities()
     OR cardinality(p_capabilities) <> (
       SELECT count(DISTINCT capability)
       FROM unnest(p_capabilities) AS capability
     ) THEN
    RAISE EXCEPTION 'capabilities exceed the caller authority or are invalid'
      USING ERRCODE = '42501';
  END IF;
  IF p_scope_refs IS NULL
     OR cardinality(p_scope_refs) = 0
     OR array_position(p_scope_refs, NULL) IS NOT NULL
     OR NOT p_scope_refs <@ brunn_auth.current_scope_refs()
     OR cardinality(p_scope_refs) <> (
       SELECT count(DISTINCT scope_ref)
       FROM unnest(p_scope_refs) AS scope_ref
     ) THEN
    RAISE EXCEPTION 'scope_refs exceed the caller authority or are invalid'
      USING ERRCODE = '42501';
  END IF;
  SELECT count(*) INTO matched_scope_count
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  IF matched_scope_count <> cardinality(p_scope_refs) THEN
    RAISE EXCEPTION 'one or more scope_refs do not belong to the user'
      USING ERRCODE = '22023';
  END IF;
  INSERT INTO brunn.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    p_user_id, p_label, p_token_hash, p_capabilities
  ) RETURNING id INTO created_credential_id;
  INSERT INTO brunn.credential_scope_grants (
    credential_id, user_id, scope_id
  )
  SELECT created_credential_id, p_user_id, scope_row.id
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  INSERT INTO brunn.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    NULL,
    brunn_auth.current_credential_id(),
    'auth.credential.issue',
    jsonb_build_object(
      'credential_id', created_credential_id,
      'capabilities', p_capabilities,
      'scope_refs', p_scope_refs
    ),
    true
  );
  RETURN created_credential_id;
END;
$$;

CREATE OR REPLACE FUNCTION brunn_auth.admin_provision_user(
  p_external_ref text,
  p_display_name text,
  p_credential_label text,
  p_token_hash text,
  p_empty_manifest_hash text
)
RETURNS TABLE (
  user_id uuid,
  credential_id uuid,
  scope_id uuid,
  policy_id uuid,
  corpus_revision_id uuid
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  owner_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'notification:publish', 'notification:manage',
    'secret:read', 'secret:write',
    'admin'
  ];
  created_user_id uuid;
  created_credential_id uuid;
  root_scope_id uuid;
  default_policy_id uuid;
  initial_revision_id uuid := gen_random_uuid();
  manifest_id uuid := gen_random_uuid();
BEGIN
  PERFORM brunn_auth.require_admin();
  IF p_external_ref IS NULL OR btrim(p_external_ref) = ''
     OR length(p_external_ref) > 200
     OR p_display_name IS NULL OR btrim(p_display_name) = ''
     OR length(p_display_name) > 200
     OR p_credential_label IS NULL OR btrim(p_credential_label) = ''
     OR length(p_credential_label) > 120 THEN
    RAISE EXCEPTION 'provisioning names are invalid' USING ERRCODE = '22023';
  END IF;
  IF EXISTS (
    SELECT 1 FROM brunn.users AS existing_user
    WHERE existing_user.external_ref = p_external_ref
  ) THEN
    RAISE EXCEPTION 'external_ref already exists' USING ERRCODE = '23505';
  END IF;
  INSERT INTO brunn.users (external_ref, display_name)
  VALUES (p_external_ref, p_display_name)
  RETURNING users.id INTO created_user_id;
  SELECT scope_row.id INTO root_scope_id
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = created_user_id
    AND scope_row.scope_ref = 'scope:root';
  SELECT policy_row.id INTO default_policy_id
  FROM brunn.policies AS policy_row
  WHERE policy_row.user_id = created_user_id
    AND policy_row.is_default;
  INSERT INTO brunn.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    created_user_id, p_credential_label, p_token_hash, owner_capabilities
  ) RETURNING api_credentials.id INTO created_credential_id;
  INSERT INTO brunn.credential_scope_grants (
    credential_id, user_id, scope_id
  ) VALUES (created_credential_id, created_user_id, root_scope_id);
  INSERT INTO brunn.corpus_revisions (
    id, user_id, scope_id, parent_revision_id, revision_number, manifest_hash
  ) VALUES (
    initial_revision_id, created_user_id, root_scope_id, NULL, 1, p_empty_manifest_hash
  );
  INSERT INTO brunn.active_manifests (
    id, user_id, scope_id, active_corpus_revision_id, manifest_hash, generation
  ) VALUES (
    manifest_id, created_user_id, root_scope_id,
    initial_revision_id, p_empty_manifest_hash, 1
  );
  INSERT INTO brunn.active_manifest_history (
    id, user_id, scope_id, manifest_id, generation, corpus_revision_id,
    manifest_hash, change_kind
  ) VALUES (
    gen_random_uuid(), created_user_id, root_scope_id, manifest_id, 1,
    initial_revision_id, p_empty_manifest_hash, 'initial'
  );
  INSERT INTO brunn.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    brunn_auth.current_user_id(),
    brunn_auth.current_credential_id(),
    'admin.user.provision',
    jsonb_build_object('target_user_id', created_user_id),
    true
  );
  RETURN QUERY SELECT
    created_user_id, created_credential_id, root_scope_id,
    default_policy_id, initial_revision_id;
END;
$$;

CREATE TABLE brunn.secrets (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES brunn.users(id) ON DELETE CASCADE,
  name text NOT NULL CHECK (name ~ '^[a-z0-9][a-z0-9._-]{0,119}$'),
  description text CHECK (description IS NULL OR length(description) BETWEEN 1 AND 1000),
  value_ciphertext bytea NOT NULL CHECK (octet_length(value_ciphertext) BETWEEN 17 AND 65552),
  value_nonce bytea NOT NULL CHECK (octet_length(value_nonce) = 12),
  version integer NOT NULL DEFAULT 1 CHECK (version > 0),
  created_by_credential_id uuid NOT NULL,
  updated_by_credential_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, name),
  FOREIGN KEY (user_id, created_by_credential_id)
    REFERENCES brunn.api_credentials(user_id, id),
  FOREIGN KEY (user_id, updated_by_credential_id)
    REFERENCES brunn.api_credentials(user_id, id)
);

-- Content-free access history. Rows intentionally survive secret deletion, so
-- secret_id has no foreign key; last_used comes from the newest 'get' row.
CREATE TABLE brunn.secret_access_log (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES brunn.users(id) ON DELETE CASCADE,
  secret_id uuid NOT NULL,
  credential_id uuid NOT NULL,
  operation text NOT NULL CHECK (operation IN ('put', 'get', 'delete')),
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES brunn.api_credentials(user_id, id)
);

CREATE INDEX secret_access_log_secret_idx
  ON brunn.secret_access_log (user_id, secret_id, operation, recorded_at DESC);

CREATE TRIGGER secret_access_log_immutable
BEFORE UPDATE OR DELETE ON brunn.secret_access_log
FOR EACH ROW EXECUTE FUNCTION brunn.prevent_immutable_mutation();

ALTER TABLE brunn.secrets ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.secrets FORCE ROW LEVEL SECURITY;
ALTER TABLE brunn.secret_access_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.secret_access_log FORCE ROW LEVEL SECURITY;

CREATE POLICY secrets_select ON brunn.secrets
  FOR SELECT USING (
    user_id = brunn_auth.current_user_id()
    AND brunn_auth.context_is_valid()
    AND brunn_auth.has_any_capability(
      ARRAY['secret:read', 'secret:write', 'admin']
    )
  );

CREATE POLICY secrets_insert ON brunn.secrets
  FOR INSERT WITH CHECK (
    user_id = brunn_auth.current_user_id()
    AND created_by_credential_id = brunn_auth.current_credential_id()
    AND updated_by_credential_id = brunn_auth.current_credential_id()
    AND brunn_auth.context_is_valid()
    AND brunn_auth.has_any_capability(ARRAY['secret:write', 'admin'])
  );

CREATE POLICY secrets_update ON brunn.secrets
  FOR UPDATE USING (
    user_id = brunn_auth.current_user_id()
    AND brunn_auth.context_is_valid()
    AND brunn_auth.has_any_capability(ARRAY['secret:write', 'admin'])
  ) WITH CHECK (
    user_id = brunn_auth.current_user_id()
    AND updated_by_credential_id = brunn_auth.current_credential_id()
    AND brunn_auth.context_is_valid()
    AND brunn_auth.has_any_capability(ARRAY['secret:write', 'admin'])
  );

CREATE POLICY secrets_delete ON brunn.secrets
  FOR DELETE USING (
    user_id = brunn_auth.current_user_id()
    AND brunn_auth.context_is_valid()
    AND brunn_auth.has_any_capability(ARRAY['secret:write', 'admin'])
  );

CREATE POLICY secret_access_log_select ON brunn.secret_access_log
  FOR SELECT USING (
    user_id = brunn_auth.current_user_id()
    AND brunn_auth.context_is_valid()
    AND brunn_auth.has_any_capability(
      ARRAY['secret:read', 'secret:write', 'admin']
    )
  );

CREATE POLICY secret_access_log_insert ON brunn.secret_access_log
  FOR INSERT WITH CHECK (
    user_id = brunn_auth.current_user_id()
    AND credential_id = brunn_auth.current_credential_id()
    AND brunn_auth.context_is_valid()
    AND brunn_auth.has_any_capability(
      ARRAY['secret:read', 'secret:write', 'admin']
    )
  );

GRANT SELECT, INSERT, UPDATE, DELETE ON brunn.secrets TO app_rw;
GRANT SELECT ON brunn.secrets TO app_ro;
GRANT SELECT, INSERT ON brunn.secret_access_log TO app_rw;
GRANT SELECT ON brunn.secret_access_log TO app_ro;

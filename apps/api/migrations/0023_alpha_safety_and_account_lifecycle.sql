ALTER TABLE straylight.users
  ADD COLUMN account_status text NOT NULL DEFAULT 'active'
    CHECK (account_status IN ('active', 'deleting', 'deleted')),
  ADD COLUMN storage_limit_bytes bigint NOT NULL DEFAULT 10737418240
    CHECK (storage_limit_bytes > 0),
  ADD COLUMN daily_embedding_input_chars bigint NOT NULL DEFAULT 50000000
    CHECK (daily_embedding_input_chars > 0),
  ADD COLUMN daily_model_input_chars bigint NOT NULL DEFAULT 10000000
    CHECK (daily_model_input_chars > 0),
  ADD COLUMN daily_model_output_tokens bigint NOT NULL DEFAULT 1000000
    CHECK (daily_model_output_tokens > 0),
  ADD COLUMN daily_compute_rows bigint NOT NULL DEFAULT 5000000
    CHECK (daily_compute_rows > 0),
  ADD COLUMN deletion_requested_at timestamptz,
  ADD COLUMN deleted_at timestamptz,
  ADD CONSTRAINT users_deletion_state_check CHECK (
    (account_status = 'active' AND deleted_at IS NULL)
    OR (account_status = 'deleting' AND deletion_requested_at IS NOT NULL AND deleted_at IS NULL)
    OR (account_status = 'deleted' AND deletion_requested_at IS NOT NULL AND deleted_at IS NOT NULL)
  );

CREATE TABLE straylight.user_usage_daily (
  user_id uuid NOT NULL REFERENCES straylight.users(id),
  usage_date date NOT NULL DEFAULT (clock_timestamp() AT TIME ZONE 'UTC')::date,
  embedding_input_chars bigint NOT NULL DEFAULT 0 CHECK (embedding_input_chars >= 0),
  model_input_chars bigint NOT NULL DEFAULT 0 CHECK (model_input_chars >= 0),
  model_output_tokens bigint NOT NULL DEFAULT 0 CHECK (model_output_tokens >= 0),
  compute_rows bigint NOT NULL DEFAULT 0 CHECK (compute_rows >= 0),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, usage_date)
);

CREATE TABLE straylight.account_exports (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  requested_by_credential_id uuid NOT NULL,
  status text NOT NULL CHECK (status IN (
    'queued', 'running', 'ready', 'failed', 'expired', 'deleted'
  )),
  object_key text,
  content_hash straylight.sha256_hex,
  size_bytes bigint CHECK (size_bytes IS NULL OR size_bytes >= 0),
  table_count integer CHECK (table_count IS NULL OR table_count >= 0),
  object_count integer CHECK (object_count IS NULL OR object_count >= 0),
  failure_code text,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  started_at timestamptz,
  completed_at timestamptz,
  expires_at timestamptz NOT NULL DEFAULT clock_timestamp() + interval '24 hours',
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, requested_by_credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  CHECK (completed_at IS NULL OR completed_at >= created_at),
  CHECK (expires_at > created_at)
);

CREATE INDEX account_exports_claim_idx
  ON straylight.account_exports (status, created_at)
  WHERE status IN ('queued', 'running');

CREATE TABLE straylight.account_deletion_requests (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  requested_by_credential_id uuid NOT NULL,
  status text NOT NULL CHECK (status IN (
    'queued', 'running', 'awaiting_backup_expiry', 'completed', 'failed', 'canceled'
  )),
  confirmation_hash straylight.sha256_hex NOT NULL,
  reason straylight.nonempty_text NOT NULL,
  records_total bigint NOT NULL DEFAULT 0 CHECK (records_total >= 0),
  records_completed bigint NOT NULL DEFAULT 0 CHECK (records_completed >= 0),
  backup_expiry_due_at timestamptz NOT NULL,
  failure_code text,
  terminal_result jsonb,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  started_at timestamptz,
  completed_at timestamptz,
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, requested_by_credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  CHECK (records_completed <= records_total),
  CHECK (completed_at IS NULL OR completed_at >= created_at)
);

CREATE UNIQUE INDEX account_deletion_one_active_idx
  ON straylight.account_deletion_requests (user_id)
  WHERE status IN ('queued', 'running', 'awaiting_backup_expiry');

CREATE INDEX account_deletion_claim_idx
  ON straylight.account_deletion_requests (status, created_at)
  WHERE status IN ('queued', 'running', 'awaiting_backup_expiry');

CREATE INDEX background_jobs_running_lease_idx
  ON straylight.background_jobs (locked_at, created_at)
  WHERE status = 'running';

ALTER TABLE straylight.user_usage_daily ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.user_usage_daily FORCE ROW LEVEL SECURITY;
ALTER TABLE straylight.account_exports ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.account_exports FORCE ROW LEVEL SECURITY;
ALTER TABLE straylight.account_deletion_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.account_deletion_requests FORCE ROW LEVEL SECURITY;

CREATE POLICY user_usage_daily_self_select
  ON straylight.user_usage_daily
  FOR SELECT TO app_rw, app_ro
  USING (straylight_auth.can_access_user(user_id));

CREATE POLICY user_usage_daily_self_insert
  ON straylight.user_usage_daily
  FOR INSERT TO app_rw
  WITH CHECK (straylight_auth.can_access_user(user_id));

CREATE POLICY user_usage_daily_self_update
  ON straylight.user_usage_daily
  FOR UPDATE TO app_rw
  USING (straylight_auth.can_access_user(user_id))
  WITH CHECK (straylight_auth.can_access_user(user_id));

CREATE POLICY account_exports_self_select
  ON straylight.account_exports
  FOR SELECT TO app_rw, app_ro
  USING (straylight_auth.can_access_user(user_id));

CREATE POLICY account_exports_self_insert
  ON straylight.account_exports
  FOR INSERT TO app_rw
  WITH CHECK (
    straylight_auth.can_access_user(user_id)
    AND straylight_auth.has_capability('status')
    AND status = 'queued'
  );

CREATE POLICY account_deletions_self_select
  ON straylight.account_deletion_requests
  FOR SELECT TO app_rw, app_ro
  USING (straylight_auth.can_access_user(user_id));

CREATE POLICY account_deletions_self_insert
  ON straylight.account_deletion_requests
  FOR INSERT TO app_rw
  WITH CHECK (
    straylight_auth.can_access_user(user_id)
    AND straylight_auth.has_capability('credential:manage')
    AND status = 'queued'
  );

GRANT SELECT, INSERT, UPDATE ON straylight.user_usage_daily TO app_rw;
GRANT SELECT ON straylight.user_usage_daily TO app_ro;
GRANT SELECT, INSERT ON straylight.account_exports TO app_rw;
GRANT SELECT ON straylight.account_exports TO app_ro;
GRANT SELECT, INSERT ON straylight.account_deletion_requests TO app_rw;
GRANT SELECT ON straylight.account_deletion_requests TO app_ro;

CREATE FUNCTION straylight_auth.require_admin()
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
BEGIN
  IF NOT straylight_auth.context_is_valid()
     OR NOT straylight_auth.has_capability('admin') THEN
    RAISE EXCEPTION 'authenticated administrative capability is required'
      USING ERRCODE = '42501';
  END IF;
END;
$$;

CREATE FUNCTION straylight_auth.admin_issue_credential(
  p_user_id uuid,
  p_label text,
  p_token_hash text,
  p_capabilities text[],
  p_scope_refs text[]
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  allowed_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage'
  ];
  created_credential_id uuid;
  matched_scope_count integer;
BEGIN
  PERFORM straylight_auth.require_admin();

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
    SELECT 1
    FROM straylight.users
    WHERE id = p_user_id AND account_status = 'active'
  ) THEN
    RAISE EXCEPTION 'active recovery user not found'
      USING ERRCODE = 'P0002';
  END IF;

  SELECT count(*) INTO matched_scope_count
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  IF matched_scope_count <> cardinality(p_scope_refs) THEN
    RAISE EXCEPTION 'one or more recovery scopes do not belong to the user'
      USING ERRCODE = '22023';
  END IF;

  INSERT INTO straylight.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    p_user_id, p_label, p_token_hash, p_capabilities
  )
  RETURNING id INTO created_credential_id;

  INSERT INTO straylight.credential_scope_grants (
    credential_id, user_id, scope_id
  )
  SELECT created_credential_id, p_user_id, scope_row.id
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);

  INSERT INTO straylight.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    straylight_auth.current_user_id(),
    straylight_auth.current_credential_id(),
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

CREATE FUNCTION straylight_auth.admin_provision_user(
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
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  owner_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage'
  ];
  created_user_id uuid;
  created_credential_id uuid;
  root_scope_id uuid;
  default_policy_id uuid;
  initial_revision_id uuid := gen_random_uuid();
  manifest_id uuid := gen_random_uuid();
BEGIN
  PERFORM straylight_auth.require_admin();

  IF p_external_ref IS NULL OR btrim(p_external_ref) = ''
     OR length(p_external_ref) > 200
     OR p_display_name IS NULL OR btrim(p_display_name) = ''
     OR length(p_display_name) > 200
     OR p_credential_label IS NULL OR btrim(p_credential_label) = ''
     OR length(p_credential_label) > 120 THEN
    RAISE EXCEPTION 'provisioning names are invalid'
      USING ERRCODE = '22023';
  END IF;

  IF EXISTS (
    SELECT 1 FROM straylight.users WHERE external_ref = p_external_ref
  ) THEN
    RAISE EXCEPTION 'external_ref already exists'
      USING ERRCODE = '23505';
  END IF;

  INSERT INTO straylight.users (external_ref, display_name)
  VALUES (p_external_ref, p_display_name)
  RETURNING id INTO created_user_id;

  SELECT id INTO root_scope_id
  FROM straylight.scopes
  WHERE user_id = created_user_id AND scope_ref = 'scope:root';

  SELECT id INTO default_policy_id
  FROM straylight.policies
  WHERE user_id = created_user_id AND is_default;

  INSERT INTO straylight.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    created_user_id, p_credential_label, p_token_hash, owner_capabilities
  )
  RETURNING id INTO created_credential_id;

  INSERT INTO straylight.credential_scope_grants (
    credential_id, user_id, scope_id
  ) VALUES (
    created_credential_id, created_user_id, root_scope_id
  );

  INSERT INTO straylight.corpus_revisions (
    id, user_id, scope_id, parent_revision_id, revision_number, manifest_hash
  ) VALUES (
    initial_revision_id, created_user_id, root_scope_id, NULL, 1, p_empty_manifest_hash
  );

  INSERT INTO straylight.active_manifests (
    id, user_id, scope_id, active_corpus_revision_id, manifest_hash, generation
  ) VALUES (
    manifest_id, created_user_id, root_scope_id,
    initial_revision_id, p_empty_manifest_hash, 1
  );

  INSERT INTO straylight.active_manifest_history (
    id, user_id, scope_id, manifest_id, generation, corpus_revision_id,
    manifest_hash, change_kind
  ) VALUES (
    gen_random_uuid(), created_user_id, root_scope_id, manifest_id, 1,
    initial_revision_id, p_empty_manifest_hash, 'initial'
  );

  INSERT INTO straylight.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    straylight_auth.current_user_id(),
    straylight_auth.current_credential_id(),
    'admin.user.provision',
    jsonb_build_object(
      'target_user_id', created_user_id,
      'external_ref', p_external_ref
    ),
    true
  );

  RETURN QUERY SELECT
    created_user_id,
    created_credential_id,
    root_scope_id,
    default_policy_id,
    initial_revision_id;
END;
$$;

REVOKE ALL ON FUNCTION straylight_auth.require_admin() FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.admin_issue_credential(
  uuid, text, text, text[], text[]
) FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.admin_provision_user(
  text, text, text, text, text
) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight_auth.require_admin() TO app_rw;
GRANT EXECUTE ON FUNCTION straylight_auth.admin_issue_credential(
  uuid, text, text, text[], text[]
) TO app_rw;
GRANT EXECUTE ON FUNCTION straylight_auth.admin_provision_user(
  text, text, text, text, text
) TO app_rw;

COMMENT ON TABLE straylight.user_usage_daily IS
  'Per-user UTC quota counters. Reservations fail closed and are never used for retrieval ranking.';
COMMENT ON TABLE straylight.account_exports IS
  'Asynchronous complete user export packages with short-lived object storage.';
COMMENT ON TABLE straylight.account_deletion_requests IS
  'Auditable account deletion orchestration including bounded backup-retention completion.';

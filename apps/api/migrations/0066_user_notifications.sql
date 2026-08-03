-- Durable user notifications, iOS installations, and an APNs delivery outbox.
-- Notification content remains in Postgres; APNs receives only opaque refs and
-- generic copy. Provider acceptance and user receipts are recorded separately.

ALTER TABLE straylight.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_capabilities_check2;

ALTER TABLE straylight.api_credentials
  ADD CONSTRAINT api_credentials_capabilities_check2 CHECK (
    capabilities <@ ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'notification:publish', 'notification:manage',
      'admin'
    ]::text[]
  );

ALTER TABLE straylight.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_owner_full_capabilities_check;

UPDATE straylight.api_credentials
SET capabilities = capabilities
  || ARRAY['notification:publish', 'notification:manage']::text[]
WHERE capabilities @> ARRAY['credential:manage']::text[]
  AND NOT capabilities @> ARRAY[
    'notification:publish', 'notification:manage'
  ]::text[];

ALTER TABLE straylight.api_credentials
  ADD CONSTRAINT api_credentials_owner_full_capabilities_check CHECK (
    NOT capabilities @> ARRAY['credential:manage']::text[]
    OR capabilities @> ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'notification:publish', 'notification:manage',
      'admin'
    ]::text[]
  );

CREATE OR REPLACE FUNCTION straylight_auth.admin_issue_credential(
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
    'credential:manage', 'notification:publish', 'notification:manage',
    'admin'
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
    SELECT 1 FROM straylight.users
    WHERE id = p_user_id AND account_status = 'active'
  ) THEN
    RAISE EXCEPTION 'active recovery user not found' USING ERRCODE = 'P0002';
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
  ) VALUES (p_user_id, p_label, p_token_hash, p_capabilities)
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

CREATE OR REPLACE FUNCTION straylight_auth.issue_credential(
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
    'credential:manage', 'notification:publish', 'notification:manage',
    'admin'
  ];
  created_credential_id uuid;
  matched_scope_count integer;
BEGIN
  PERFORM straylight_auth.require_credential_control(p_user_id);
  IF p_capabilities IS NULL
     OR cardinality(p_capabilities) = 0
     OR array_position(p_capabilities, NULL) IS NOT NULL
     OR NOT p_capabilities <@ allowed_capabilities
     OR NOT p_capabilities <@ straylight_auth.current_capabilities()
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
     OR NOT p_scope_refs <@ straylight_auth.current_scope_refs()
     OR cardinality(p_scope_refs) <> (
       SELECT count(DISTINCT scope_ref)
       FROM unnest(p_scope_refs) AS scope_ref
     ) THEN
    RAISE EXCEPTION 'scope_refs exceed the caller authority or are invalid'
      USING ERRCODE = '42501';
  END IF;
  SELECT count(*) INTO matched_scope_count
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  IF matched_scope_count <> cardinality(p_scope_refs) THEN
    RAISE EXCEPTION 'one or more scope_refs do not belong to the user'
      USING ERRCODE = '22023';
  END IF;
  INSERT INTO straylight.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    p_user_id, p_label, p_token_hash, p_capabilities
  ) RETURNING id INTO created_credential_id;
  INSERT INTO straylight.credential_scope_grants (
    credential_id, user_id, scope_id
  )
  SELECT created_credential_id, p_user_id, scope_row.id
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  INSERT INTO straylight.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    NULL,
    straylight_auth.current_credential_id(),
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

CREATE OR REPLACE FUNCTION straylight_auth.admin_provision_user(
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
    'credential:manage', 'notification:publish', 'notification:manage',
    'admin'
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
    RAISE EXCEPTION 'provisioning names are invalid' USING ERRCODE = '22023';
  END IF;
  IF EXISTS (
    SELECT 1 FROM straylight.users AS existing_user
    WHERE existing_user.external_ref = p_external_ref
  ) THEN
    RAISE EXCEPTION 'external_ref already exists' USING ERRCODE = '23505';
  END IF;
  INSERT INTO straylight.users (external_ref, display_name)
  VALUES (p_external_ref, p_display_name)
  RETURNING users.id INTO created_user_id;
  SELECT scope_row.id INTO root_scope_id
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = created_user_id
    AND scope_row.scope_ref = 'scope:root';
  SELECT policy_row.id INTO default_policy_id
  FROM straylight.policies AS policy_row
  WHERE policy_row.user_id = created_user_id
    AND policy_row.is_default;
  INSERT INTO straylight.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    created_user_id, p_credential_label, p_token_hash, owner_capabilities
  ) RETURNING api_credentials.id INTO created_credential_id;
  INSERT INTO straylight.credential_scope_grants (
    credential_id, user_id, scope_id
  ) VALUES (created_credential_id, created_user_id, root_scope_id);
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
    jsonb_build_object('target_user_id', created_user_id),
    true
  );
  RETURN QUERY SELECT
    created_user_id, created_credential_id, root_scope_id,
    default_policy_id, initial_revision_id;
END;
$$;

CREATE TABLE straylight.notifications (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  producer_credential_id uuid NOT NULL,
  event_key text NOT NULL CHECK (length(event_key) BETWEEN 1 AND 200),
  request_hash text NOT NULL CHECK (request_hash ~ '^[0-9a-f]{64}$'),
  correlation_id text NOT NULL CHECK (length(correlation_id) BETWEEN 1 AND 200),
  kind text NOT NULL CHECK (
    kind IN ('briefing_ready', 'news_alert', 'correction', 'operational')
  ),
  importance text NOT NULL CHECK (importance IN ('normal', 'important')),
  title text NOT NULL CHECK (length(title) BETWEEN 1 AND 240),
  body text NOT NULL CHECK (length(body) BETWEEN 1 AND 20000),
  source jsonb CHECK (source IS NULL OR jsonb_typeof(source) = 'object'),
  target jsonb NOT NULL CHECK (jsonb_typeof(target) = 'object'),
  occurred_at timestamptz NOT NULL,
  expires_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, event_key),
  FOREIGN KEY (user_id, producer_credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  CHECK (expires_at > occurred_at)
);

CREATE INDEX notifications_timeline_idx
  ON straylight.notifications (user_id, created_at DESC, id DESC);

CREATE TABLE straylight.notification_user_state (
  user_id uuid NOT NULL,
  notification_id uuid NOT NULL,
  opened_at timestamptz,
  acknowledged_at timestamptz,
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, notification_id),
  FOREIGN KEY (user_id, notification_id)
    REFERENCES straylight.notifications(user_id, id) ON DELETE CASCADE,
  CHECK (acknowledged_at IS NULL OR opened_at IS NOT NULL)
);

CREATE TABLE straylight.notification_installations (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  client_installation_id uuid NOT NULL,
  registered_by_credential_id uuid NOT NULL,
  platform text NOT NULL CHECK (platform = 'ios'),
  environment text NOT NULL CHECK (environment IN ('development', 'production')),
  app_id text NOT NULL CHECK (length(app_id) BETWEEN 3 AND 255),
  token_ciphertext bytea CHECK (
    token_ciphertext IS NULL OR octet_length(token_ciphertext) BETWEEN 32 AND 1024
  ),
  token_nonce bytea CHECK (token_nonce IS NULL OR octet_length(token_nonce) = 12),
  token_hash text CHECK (token_hash IS NULL OR token_hash ~ '^[0-9a-f]{64}$'),
  preview text NOT NULL CHECK (preview = 'generic'),
  enabled boolean NOT NULL DEFAULT true,
  registered_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  last_seen_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  revoked_at timestamptz,
  UNIQUE (user_id, id),
  UNIQUE (user_id, client_installation_id),
  FOREIGN KEY (user_id, registered_by_credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  CHECK (
    (
      enabled AND revoked_at IS NULL
      AND token_ciphertext IS NOT NULL
      AND token_nonce IS NOT NULL
      AND token_hash IS NOT NULL
    ) OR (
      NOT enabled AND revoked_at IS NOT NULL
      AND token_ciphertext IS NULL
      AND token_nonce IS NULL
      AND token_hash IS NULL
    )
  )
);

CREATE UNIQUE INDEX notification_installations_live_token_idx
  ON straylight.notification_installations (
    environment, app_id, token_hash
  ) WHERE enabled;

CREATE TABLE straylight.notification_deliveries (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  notification_id uuid NOT NULL,
  installation_id uuid NOT NULL,
  state text NOT NULL DEFAULT 'queued' CHECK (
    state IN (
      'suppressed', 'queued', 'running', 'accepted_by_apns', 'failed', 'expired'
    )
  ),
  available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  lease_expires_at timestamptz,
  attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
  provider_block_count integer NOT NULL DEFAULT 0 CHECK (provider_block_count >= 0),
  max_attempts integer NOT NULL DEFAULT 8 CHECK (max_attempts BETWEEN 1 AND 20),
  accepted_at timestamptz,
  failed_at timestamptz,
  last_attempt_at timestamptz,
  last_error_code text CHECK (last_error_code IS NULL OR length(last_error_code) <= 120),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, id, notification_id),
  UNIQUE (user_id, notification_id, installation_id),
  FOREIGN KEY (user_id, notification_id)
    REFERENCES straylight.notifications(user_id, id) ON DELETE CASCADE,
  FOREIGN KEY (user_id, installation_id)
    REFERENCES straylight.notification_installations(user_id, id) ON DELETE CASCADE
);

CREATE INDEX notification_deliveries_due_idx
  ON straylight.notification_deliveries (available_at, created_at, id)
  WHERE state = 'queued';

CREATE INDEX notification_deliveries_stale_lease_idx
  ON straylight.notification_deliveries (lease_expires_at, id)
  WHERE state = 'running';

CREATE TABLE straylight.notification_attempts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  delivery_id uuid NOT NULL,
  attempt_number integer NOT NULL CHECK (attempt_number > 0),
  result text NOT NULL CHECK (
    result IN ('accepted_by_apns', 'retryable_failure', 'permanent_failure')
  ),
  provider_status integer CHECK (provider_status IS NULL OR provider_status BETWEEN 100 AND 599),
  provider_request_id text CHECK (
    provider_request_id IS NULL OR length(provider_request_id) <= 200
  ),
  error_code text CHECK (error_code IS NULL OR length(error_code) <= 120),
  attempted_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, delivery_id, attempt_number),
  FOREIGN KEY (user_id, delivery_id)
    REFERENCES straylight.notification_deliveries(user_id, id) ON DELETE CASCADE
);

CREATE TABLE straylight.notification_receipts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  notification_id uuid NOT NULL,
  delivery_id uuid,
  kind text NOT NULL CHECK (kind IN ('opened', 'acknowledged')),
  recorded_by_credential_id uuid NOT NULL,
  recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  FOREIGN KEY (user_id, notification_id)
    REFERENCES straylight.notifications(user_id, id) ON DELETE CASCADE,
  FOREIGN KEY (user_id, delivery_id, notification_id)
    REFERENCES straylight.notification_deliveries(user_id, id, notification_id)
    ON DELETE CASCADE,
  FOREIGN KEY (user_id, recorded_by_credential_id)
    REFERENCES straylight.api_credentials(user_id, id)
);

CREATE UNIQUE INDEX notification_receipts_without_delivery_idx
  ON straylight.notification_receipts (user_id, notification_id, kind)
  WHERE delivery_id IS NULL;

CREATE UNIQUE INDEX notification_receipts_with_delivery_idx
  ON straylight.notification_receipts (user_id, notification_id, delivery_id, kind)
  WHERE delivery_id IS NOT NULL;

CREATE FUNCTION straylight.claim_notification_device_token(
  p_client_installation_id uuid,
  p_environment text,
  p_app_id text,
  p_token_hash text
)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  actor_user_id uuid := straylight_auth.current_user_id();
  reassigned_count bigint;
BEGIN
  IF NOT straylight_auth.context_is_valid()
     OR actor_user_id IS NULL
     OR NOT (
       straylight_auth.has_capability('notification:manage')
       OR straylight_auth.has_capability('admin')
     ) THEN
    RAISE EXCEPTION 'notification management capability is required'
      USING ERRCODE = '42501';
  END IF;
  IF p_client_installation_id IS NULL
     OR p_environment NOT IN ('development', 'production')
     OR p_app_id IS NULL OR length(p_app_id) NOT BETWEEN 3 AND 255
     OR p_token_hash IS NULL OR p_token_hash !~ '^[0-9a-f]{64}$' THEN
    RAISE EXCEPTION 'notification device-token claim is invalid'
      USING ERRCODE = '22023';
  END IF;

  -- Serialize claims for one provider token so the global live-token unique
  -- index cannot race two account switches into an ambiguous assignment.
  PERFORM pg_advisory_xact_lock(hashtextextended(
    'straylight.notification-token.v1|'
      || p_environment || '|' || p_app_id || '|' || p_token_hash,
    0
  ));

  WITH revoked AS (
    UPDATE straylight.notification_installations
    SET enabled=false,
        token_ciphertext=NULL,
        token_nonce=NULL,
        token_hash=NULL,
        revoked_at=coalesce(revoked_at,clock_timestamp()),
        updated_at=clock_timestamp()
    WHERE enabled
      AND environment=p_environment
      AND app_id=p_app_id
      AND token_hash=p_token_hash
      AND NOT (
        user_id=actor_user_id
        AND client_installation_id=p_client_installation_id
      )
    RETURNING user_id,id
  ), expired AS (
    UPDATE straylight.notification_deliveries AS delivery
    SET state='expired',failed_at=clock_timestamp(),lease_expires_at=NULL,
        last_error_code='installation_reassigned',updated_at=clock_timestamp()
    FROM revoked
    WHERE delivery.user_id=revoked.user_id
      AND delivery.installation_id=revoked.id
      AND delivery.state IN ('queued','running')
    RETURNING delivery.id
  )
  SELECT count(*) INTO reassigned_count FROM revoked;

  INSERT INTO straylight.audit_events (
    user_id,credential_id,action,details,content_free
  ) VALUES (
    actor_user_id,
    straylight_auth.current_credential_id(),
    'notifications.installation.token_claim',
    jsonb_build_object('reassigned_installations',reassigned_count),
    true
  );
  RETURN reassigned_count;
END;
$$;

REVOKE ALL ON FUNCTION straylight.claim_notification_device_token(
  uuid,text,text,text
) FROM PUBLIC, app_ro;
GRANT EXECUTE ON FUNCTION straylight.claim_notification_device_token(
  uuid,text,text,text
) TO app_rw;

CREATE FUNCTION straylight.revoke_disabled_credential_notification_installations()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
BEGIN
  IF OLD.disabled_at IS NULL AND NEW.disabled_at IS NOT NULL THEN
    WITH revoked AS (
      UPDATE straylight.notification_installations
      SET enabled=false,revoked_at=coalesce(revoked_at,clock_timestamp()),
          token_ciphertext=NULL,token_nonce=NULL,token_hash=NULL,
          updated_at=clock_timestamp()
      WHERE user_id=NEW.user_id
        AND registered_by_credential_id=NEW.id
        AND enabled
      RETURNING id
    )
    UPDATE straylight.notification_deliveries
    SET state='expired',failed_at=clock_timestamp(),lease_expires_at=NULL,
        last_error_code='registration_credential_revoked',
        updated_at=clock_timestamp()
    WHERE user_id=NEW.user_id
      AND installation_id IN (SELECT id FROM revoked)
      AND state IN ('queued','running');
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER api_credentials_revoke_notification_installations
AFTER UPDATE OF disabled_at ON straylight.api_credentials
FOR EACH ROW
EXECUTE FUNCTION straylight.revoke_disabled_credential_notification_installations();

REVOKE ALL ON FUNCTION
  straylight.revoke_disabled_credential_notification_installations()
FROM PUBLIC, app_rw, app_ro;

DO $$
DECLARE
  table_name text;
BEGIN
  FOREACH table_name IN ARRAY ARRAY[
    'notifications', 'notification_user_state', 'notification_installations',
    'notification_deliveries', 'notification_attempts', 'notification_receipts'
  ] LOOP
    EXECUTE format('ALTER TABLE straylight.%I ENABLE ROW LEVEL SECURITY', table_name);
    EXECUTE format('ALTER TABLE straylight.%I FORCE ROW LEVEL SECURITY', table_name);
    EXECUTE format(
      'CREATE POLICY notification_user_select ON straylight.%I '
      'FOR SELECT USING ('
      'user_id = straylight_auth.current_user_id() '
      'AND straylight_auth.context_is_valid() '
      'AND straylight_auth.has_capability(''read''))',
      table_name
    );
  END LOOP;
END;
$$;

CREATE POLICY notifications_insert ON straylight.notifications
  FOR INSERT WITH CHECK (
    user_id = straylight_auth.current_user_id()
    AND straylight_auth.context_is_valid()
    AND producer_credential_id = straylight_auth.current_credential_id()
    AND (
      straylight_auth.has_capability('notification:publish')
      OR straylight_auth.has_capability('save')
      OR straylight_auth.has_capability('admin')
    )
  );

CREATE POLICY notification_state_write ON straylight.notification_user_state
  FOR ALL USING (
    user_id = straylight_auth.current_user_id()
    AND straylight_auth.context_is_valid()
    AND (
      straylight_auth.has_capability('notification:manage')
      OR straylight_auth.has_capability('admin')
    )
  ) WITH CHECK (
    user_id = straylight_auth.current_user_id()
    AND straylight_auth.context_is_valid()
    AND (
      straylight_auth.has_capability('notification:manage')
      OR straylight_auth.has_capability('admin')
    )
  );

CREATE POLICY notification_installations_insert ON straylight.notification_installations
  FOR INSERT WITH CHECK (
    user_id = straylight_auth.current_user_id()
    AND registered_by_credential_id = straylight_auth.current_credential_id()
    AND straylight_auth.context_is_valid()
    AND (
      straylight_auth.has_capability('notification:manage')
      OR straylight_auth.has_capability('admin')
    )
  );

CREATE POLICY notification_installations_update ON straylight.notification_installations
  FOR UPDATE USING (
    user_id = straylight_auth.current_user_id()
    AND straylight_auth.context_is_valid()
    AND (
      straylight_auth.has_capability('notification:manage')
      OR straylight_auth.has_capability('admin')
    )
  ) WITH CHECK (
    user_id = straylight_auth.current_user_id()
    AND straylight_auth.context_is_valid()
    AND (
      straylight_auth.has_capability('notification:manage')
      OR straylight_auth.has_capability('admin')
    )
  );

CREATE POLICY notification_deliveries_publish ON straylight.notification_deliveries
  FOR INSERT WITH CHECK (
    user_id = straylight_auth.current_user_id()
    AND straylight_auth.context_is_valid()
    AND (
      straylight_auth.has_capability('notification:publish')
      OR straylight_auth.has_capability('save')
      OR straylight_auth.has_capability('admin')
    )
  );

CREATE POLICY notification_deliveries_installation_expire ON straylight.notification_deliveries
  FOR UPDATE USING (
    user_id = straylight_auth.current_user_id()
    AND straylight_auth.context_is_valid()
    AND (
      straylight_auth.has_capability('notification:manage')
      OR straylight_auth.has_capability('admin')
    )
  ) WITH CHECK (
    user_id = straylight_auth.current_user_id()
    AND straylight_auth.context_is_valid()
    AND (
      straylight_auth.has_capability('notification:manage')
      OR straylight_auth.has_capability('admin')
    )
  );

CREATE POLICY notification_receipts_insert ON straylight.notification_receipts
  FOR INSERT WITH CHECK (
    user_id = straylight_auth.current_user_id()
    AND recorded_by_credential_id = straylight_auth.current_credential_id()
    AND straylight_auth.context_is_valid()
    AND (
      straylight_auth.has_capability('notification:manage')
      OR straylight_auth.has_capability('admin')
    )
  );

GRANT SELECT, INSERT ON straylight.notifications TO app_rw;
GRANT SELECT ON straylight.notifications TO app_ro;
GRANT SELECT, INSERT, UPDATE ON straylight.notification_user_state TO app_rw;
GRANT SELECT ON straylight.notification_user_state TO app_ro;
GRANT SELECT, INSERT, UPDATE ON straylight.notification_installations TO app_rw;
GRANT SELECT ON straylight.notification_installations TO app_ro;
GRANT SELECT, INSERT, UPDATE ON straylight.notification_deliveries TO app_rw;
GRANT SELECT ON straylight.notification_deliveries TO app_ro;
GRANT SELECT ON straylight.notification_attempts TO app_rw, app_ro;
GRANT SELECT, INSERT ON straylight.notification_receipts TO app_rw;
GRANT SELECT ON straylight.notification_receipts TO app_ro;

-- Close alpha data-safety gaps without changing read, retrieval, capture, or
-- dreaming semantics.

CREATE OR REPLACE FUNCTION straylight_auth.authenticate_credential(p_token_hash text)
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
  JOIN straylight.users AS user_row
    ON user_row.id = credential.user_id
  LEFT JOIN straylight.credential_scope_grants AS grant_row
    ON grant_row.user_id = credential.user_id
   AND grant_row.credential_id = credential.id
  LEFT JOIN straylight.scopes AS scope
    ON scope.user_id = grant_row.user_id
   AND scope.id = grant_row.scope_id
  WHERE credential.token_hash = p_token_hash
    AND credential.disabled_at IS NULL
    AND (
      user_row.account_status = 'active'
      OR (
        user_row.account_status = 'deleting'
        AND credential.capabilities = ARRAY['status']::text[]
        AND EXISTS (
          SELECT 1
          FROM straylight.account_deletion_requests AS deletion
          WHERE deletion.user_id = credential.user_id
            AND deletion.requested_by_credential_id = credential.id
            AND deletion.status IN (
              'queued', 'running', 'awaiting_backup_expiry', 'failed'
            )
        )
      )
    )
  GROUP BY credential.id, credential.user_id, credential.capabilities
$$;

CREATE OR REPLACE FUNCTION straylight_auth.validate_transaction_context(
  p_user_id uuid,
  p_credential_id uuid,
  p_capabilities text[],
  p_scope_refs text[]
)
RETURNS TABLE(valid boolean, scope_ids uuid[])
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
  WITH credential AS (
    SELECT api.capabilities
    FROM straylight.api_credentials AS api
    JOIN straylight.users AS user_row ON user_row.id = api.user_id
    WHERE api.id = p_credential_id
      AND api.user_id = p_user_id
      AND api.disabled_at IS NULL
      AND (
        user_row.account_status = 'active'
        OR (
          user_row.account_status = 'deleting'
          AND api.capabilities = ARRAY['status']::text[]
          AND EXISTS (
            SELECT 1
            FROM straylight.account_deletion_requests AS deletion
            WHERE deletion.user_id = api.user_id
              AND deletion.requested_by_credential_id = api.id
              AND deletion.status IN (
                'queued', 'running', 'awaiting_backup_expiry', 'failed'
              )
          )
        )
      )
  ), authorized AS (
    SELECT
      coalesce(array_agg(scope.id ORDER BY scope.id), '{}'::uuid[]) AS scope_ids,
      coalesce(
        array_agg(scope.scope_ref::text ORDER BY scope.scope_ref),
        '{}'::text[]
      ) AS scope_refs
    FROM straylight.credential_scope_grants AS grant_row
    JOIN straylight.scopes AS scope
      ON scope.user_id = grant_row.user_id
     AND scope.id = grant_row.scope_id
    WHERE grant_row.user_id = p_user_id
      AND grant_row.credential_id = p_credential_id
  ), decision AS (
    SELECT EXISTS (
      SELECT 1
      FROM credential, authorized
      WHERE p_capabilities <@ credential.capabilities
        AND p_scope_refs <@ authorized.scope_refs
    ) AS valid,
    authorized.scope_ids
    FROM authorized
  )
  SELECT decision.valid,
         CASE WHEN decision.valid THEN decision.scope_ids ELSE '{}'::uuid[] END
  FROM decision
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
    'credential:manage', 'admin'
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

CREATE OR REPLACE FUNCTION straylight_auth.request_account_deletion(
  p_user_id uuid,
  p_confirmation text,
  p_reason text,
  p_backup_retention_days integer
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  expected_confirmation text;
  request_id uuid := gen_random_uuid();
  requesting_credential_id uuid := straylight_auth.current_credential_id();
BEGIN
  PERFORM straylight_auth.require_credential_control(p_user_id);

  SELECT 'DELETE ' || external_ref
  INTO expected_confirmation
  FROM straylight.users
  WHERE id = p_user_id AND account_status = 'active'
  FOR UPDATE;

  IF expected_confirmation IS NULL THEN
    RAISE EXCEPTION 'active user not found' USING ERRCODE = 'P0002';
  END IF;
  IF p_confirmation IS DISTINCT FROM expected_confirmation THEN
    RAISE EXCEPTION 'account deletion confirmation does not match'
      USING ERRCODE = '22023';
  END IF;
  IF p_reason IS NULL OR btrim(p_reason) = '' OR length(p_reason) > 500 THEN
    RAISE EXCEPTION 'account deletion reason must contain 1 to 500 characters'
      USING ERRCODE = '22023';
  END IF;
  IF p_backup_retention_days < 1 OR p_backup_retention_days > 90 THEN
    RAISE EXCEPTION 'backup retention must be between 1 and 90 days'
      USING ERRCODE = '22023';
  END IF;
  IF EXISTS (
    SELECT 1
    FROM straylight.account_exports
    WHERE user_id = p_user_id AND status = 'running'
  ) THEN
    RAISE EXCEPTION 'a running account export must finish before deletion'
      USING ERRCODE = '55000';
  END IF;

  INSERT INTO straylight.account_deletion_requests (
    id, user_id, requested_by_credential_id, status, confirmation_hash,
    reason, backup_expiry_due_at
  ) VALUES (
    request_id,
    p_user_id,
    requesting_credential_id,
    'queued',
    encode(public.digest(p_confirmation, 'sha256'), 'hex'),
    p_reason,
    clock_timestamp() + make_interval(days => p_backup_retention_days)
  );

  UPDATE straylight.api_credentials
  SET disabled_at = coalesce(disabled_at, clock_timestamp())
  WHERE user_id = p_user_id
    AND id <> requesting_credential_id
    AND disabled_at IS NULL;

  UPDATE straylight.api_credentials
  SET capabilities = ARRAY['status']::text[]
  WHERE user_id = p_user_id
    AND id = requesting_credential_id
    AND disabled_at IS NULL;

  UPDATE straylight.users
  SET account_status = 'deleting',
      deletion_requested_at = clock_timestamp()
  WHERE id = p_user_id;

  UPDATE straylight.account_exports
  SET status = 'deleted',
      completed_at = coalesce(completed_at, clock_timestamp())
  WHERE user_id = p_user_id AND status IN ('queued', 'ready');

  INSERT INTO straylight.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    requesting_credential_id,
    'account.deletion.request',
    jsonb_build_object(
      'request_id', request_id,
      'backup_retention_days', p_backup_retention_days,
      'credential_mode', 'status_only'
    ),
    true
  );

  RETURN request_id;
END;
$$;

CREATE OR REPLACE FUNCTION straylight.purge_account_user_rows(p_user_id uuid)
RETURNS jsonb
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
DECLARE
  table_row record;
  deleted_rows bigint;
  remaining_rows bigint;
  result jsonb := '{}'::jsonb;
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM straylight.users
    WHERE id = p_user_id AND account_status = 'deleting'
  ) THEN
    RAISE EXCEPTION 'account purge requires a deleting user'
      USING ERRCODE = '55000';
  END IF;

  PERFORM set_config('session_replication_role', 'replica', true);
  FOR table_row IN
    SELECT column_row.table_name
    FROM information_schema.columns AS column_row
    JOIN information_schema.tables AS table_row
      ON table_row.table_schema = column_row.table_schema
     AND table_row.table_name = column_row.table_name
     AND table_row.table_type = 'BASE TABLE'
    WHERE column_row.table_schema = 'straylight'
      AND column_row.column_name = 'user_id'
      AND column_row.table_name <> ALL(ARRAY[
        'users', 'api_credentials', 'account_deletion_requests'
      ]::text[])
    ORDER BY column_row.table_name
  LOOP
    EXECUTE format(
      'DELETE FROM straylight.%I WHERE user_id=$1',
      table_row.table_name
    ) USING p_user_id;
    GET DIAGNOSTICS deleted_rows = ROW_COUNT;
    result := result || jsonb_build_object(table_row.table_name, deleted_rows);
  END LOOP;
  PERFORM set_config('session_replication_role', 'origin', true);

  FOR table_row IN
    SELECT column_row.table_name
    FROM information_schema.columns AS column_row
    JOIN information_schema.tables AS table_row
      ON table_row.table_schema = column_row.table_schema
     AND table_row.table_name = column_row.table_name
     AND table_row.table_type = 'BASE TABLE'
    WHERE column_row.table_schema = 'straylight'
      AND column_row.column_name = 'user_id'
      AND column_row.table_name <> ALL(ARRAY[
        'users', 'api_credentials', 'account_deletion_requests'
      ]::text[])
    ORDER BY column_row.table_name
  LOOP
    EXECUTE format(
      'SELECT count(*) FROM straylight.%I WHERE user_id=$1',
      table_row.table_name
    ) INTO remaining_rows USING p_user_id;
    IF remaining_rows <> 0 THEN
      RAISE EXCEPTION '% rows remain in %.% after account purge',
        remaining_rows, 'straylight', table_row.table_name
        USING ERRCODE = '55000';
    END IF;
  END LOOP;

  RETURN result;
EXCEPTION WHEN OTHERS THEN
  PERFORM set_config('session_replication_role', 'origin', true);
  RAISE;
END;
$$;

REVOKE ALL ON FUNCTION straylight.purge_account_user_rows(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight.purge_account_user_rows(uuid) FROM app_rw, app_ro;

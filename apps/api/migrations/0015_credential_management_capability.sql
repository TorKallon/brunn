ALTER TABLE straylight.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_capabilities_check2;

ALTER TABLE straylight.api_credentials
  ADD CONSTRAINT api_credentials_capabilities_check2 CHECK (
    capabilities <@ ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'admin'
    ]::text[]
  );

CREATE FUNCTION straylight_auth.require_credential_control(p_user_id uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
BEGIN
  IF NOT straylight_auth.context_is_valid()
     OR straylight_auth.current_user_id() IS DISTINCT FROM p_user_id
     OR NOT straylight_auth.has_capability('credential:manage') THEN
    RAISE EXCEPTION 'authenticated same-user credential management capability is required'
      USING ERRCODE = '42501';
  END IF;
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
     OR cardinality(p_capabilities) <> (
       SELECT count(DISTINCT capability)
       FROM unnest(p_capabilities) AS capability
     ) THEN
    RAISE EXCEPTION 'capabilities contain an unknown, null, or duplicate value'
      USING ERRCODE = '22023';
  END IF;

  IF p_scope_refs IS NULL
     OR cardinality(p_scope_refs) = 0
     OR array_position(p_scope_refs, NULL) IS NOT NULL
     OR cardinality(p_scope_refs) <> (
       SELECT count(DISTINCT scope_ref)
       FROM unnest(p_scope_refs) AS scope_ref
     ) THEN
    RAISE EXCEPTION 'scope_refs must be a nonempty set'
      USING ERRCODE = '22023';
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

CREATE OR REPLACE FUNCTION straylight_auth.revoke_credential(
  p_user_id uuid,
  p_credential_id uuid
)
RETURNS timestamptz
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  revoked_at timestamptz;
BEGIN
  PERFORM straylight_auth.require_credential_control(p_user_id);

  UPDATE straylight.api_credentials AS credential
  SET disabled_at = coalesce(credential.disabled_at, clock_timestamp())
  WHERE credential.user_id = p_user_id
    AND credential.id = p_credential_id
  RETURNING credential.disabled_at INTO revoked_at;

  IF revoked_at IS NULL THEN
    RAISE EXCEPTION 'credential not found for user' USING ERRCODE = 'P0002';
  END IF;

  INSERT INTO straylight.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    NULL,
    straylight_auth.current_credential_id(),
    'auth.credential.revoke',
    jsonb_build_object('credential_id', p_credential_id, 'revoked_at', revoked_at),
    true
  );

  RETURN revoked_at;
END;
$$;

REVOKE ALL ON FUNCTION straylight_auth.require_credential_control(uuid) FROM PUBLIC;

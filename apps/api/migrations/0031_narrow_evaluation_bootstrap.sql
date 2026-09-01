-- Restore native evaluation imports without reopening general user bootstrap
-- authority to the application role.

CREATE OR REPLACE FUNCTION brunn_auth.bootstrap_evaluation_user(
  p_external_ref text,
  p_display_name text,
  p_credential_label text,
  p_token_hash text,
  p_capabilities text[]
)
RETURNS TABLE (
  user_id uuid,
  credential_id uuid,
  scope_id uuid,
  policy_id uuid
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  allowed_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream'
  ];
  caller_user_id uuid := brunn_auth.current_user_id();
  caller_credential_id uuid := brunn_auth.current_credential_id();
  existing_user_id uuid;
  existing_credential_id uuid;
  existing_credential_user_id uuid;
BEGIN
  IF p_external_ref !~ '^eval-user:[0-9a-f]{64}$'
     OR p_display_name NOT LIKE 'Brunn evaluation: %'
     OR p_credential_label NOT IN (
       'Evaluation provisioning',
       'Evaluation read-only',
       'Evaluation read/write'
     )
     OR p_token_hash !~ '^[0-9a-f]{64}$'
     OR p_capabilities IS NULL
     OR cardinality(p_capabilities) = 0
     OR array_position(p_capabilities, NULL) IS NOT NULL
     OR NOT p_capabilities <@ allowed_capabilities
     OR cardinality(p_capabilities) <> (
       SELECT count(DISTINCT capability)
       FROM unnest(p_capabilities) AS capability
     ) THEN
    RAISE EXCEPTION 'invalid evaluation bootstrap request'
      USING ERRCODE = '22023';
  END IF;

  SELECT id INTO existing_user_id
  FROM brunn.users
  WHERE external_ref = p_external_ref;

  SELECT id, user_id INTO existing_credential_id, existing_credential_user_id
  FROM brunn.api_credentials
  WHERE token_hash = p_token_hash;

  IF NOT brunn_auth.has_any_capability(ARRAY['admin'])
     AND NOT coalesce(
       existing_user_id = caller_user_id
       AND existing_credential_id = caller_credential_id
       AND existing_credential_user_id = caller_user_id,
       false
     ) THEN
    RAISE EXCEPTION 'evaluation bootstrap requires an admin or its own credential'
      USING ERRCODE = '42501';
  END IF;

  RETURN QUERY
  SELECT *
  FROM brunn_auth.bootstrap_user(
    p_external_ref,
    p_display_name,
    p_credential_label,
    p_token_hash,
    p_capabilities
  );
END;
$$;

REVOKE ALL ON FUNCTION brunn_auth.bootstrap_evaluation_user(
  text, text, text, text, text[]
) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION brunn_auth.bootstrap_evaluation_user(
  text, text, text, text, text[]
) TO app_rw;

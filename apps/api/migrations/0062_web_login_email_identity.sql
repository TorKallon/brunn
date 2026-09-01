-- Email is the canonical user-facing sign-in identifier. Keep the existing
-- five-argument function signature for rolling compatibility with APIs that
-- still submit the legacy internal username, but atomically pin whichever
-- identity alias was verified before the expensive password check.

CREATE OR REPLACE FUNCTION brunn_auth.create_web_session(
  p_user_id uuid,
  p_token_hash text,
  p_expires_at timestamptz,
  p_verified_password_hash text,
  p_expected_username text
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  principal_id uuid;
  created_session_id uuid;
BEGIN
  IF p_token_hash !~ '^[0-9a-f]{64}$'
     OR p_verified_password_hash NOT LIKE '$argon2id$%'
     OR p_expires_at <= clock_timestamp()
     OR p_expires_at > clock_timestamp() + interval '30 days 1 minute' THEN
    RAISE EXCEPTION 'web session parameters are invalid' USING ERRCODE = '22023';
  END IF;

  SELECT credential.id INTO principal_id
  FROM brunn.api_credentials AS credential
  JOIN brunn.web_identities AS identity
    ON credential.user_id = identity.user_id
   AND credential.id = identity.web_credential_id
  JOIN brunn.users AS user_row ON user_row.id = identity.user_id
  WHERE identity.user_id = p_user_id
    AND user_row.account_status = 'active'
    AND credential.disabled_at IS NULL
  FOR UPDATE OF credential;

  IF principal_id IS NULL THEN
    RAISE EXCEPTION 'active web identity not found' USING ERRCODE = 'P0002';
  END IF;

  PERFORM 1
  FROM brunn.web_identities AS identity
  WHERE identity.user_id = p_user_id
    AND identity.web_credential_id = principal_id
    AND identity.password_hash = p_verified_password_hash
    AND (
      identity.username_normalized = lower(btrim(p_expected_username))
      OR identity.email_normalized = lower(btrim(p_expected_username))
    )
  FOR UPDATE;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'verified password or login identity changed' USING ERRCODE = 'P0002';
  END IF;

  DELETE FROM brunn.web_sessions
  WHERE user_id = p_user_id
    AND (revoked_at IS NOT NULL OR expires_at <= clock_timestamp());

  INSERT INTO brunn.web_sessions (
    user_id, credential_id, token_hash, expires_at
  ) VALUES (
    p_user_id, principal_id, p_token_hash, p_expires_at
  ) RETURNING id INTO created_session_id;

  INSERT INTO brunn.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id, principal_id, 'auth.web.login',
    jsonb_build_object('web_session_id', created_session_id), true
  );

  RETURN created_session_id;
END;
$$;

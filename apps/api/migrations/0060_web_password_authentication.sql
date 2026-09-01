-- Browser authentication is deliberately separate from long-lived agent bearer
-- credentials. Passwords use application-generated Argon2id PHC strings; only
-- SHA-256 hashes of random session and reset secrets reach PostgreSQL.

CREATE TABLE brunn.web_identities (
  user_id uuid PRIMARY KEY REFERENCES brunn.users(id),
  username brunn.nonempty_text NOT NULL,
  username_normalized brunn.nonempty_text NOT NULL UNIQUE,
  email brunn.nonempty_text NOT NULL,
  email_normalized brunn.nonempty_text NOT NULL UNIQUE,
  password_hash text,
  web_credential_id uuid NOT NULL UNIQUE,
  configured_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  FOREIGN KEY (user_id, web_credential_id)
    REFERENCES brunn.api_credentials(user_id, id),
  CHECK (username_normalized = lower(btrim(username::text))),
  CHECK (email_normalized = lower(btrim(email::text))),
  CHECK (username_normalized ~ '^[a-z0-9][a-z0-9._-]{2,63}$'),
  CHECK (email_normalized ~ '^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$'),
  CHECK (password_hash IS NULL OR password_hash LIKE '$argon2id$%')
);

CREATE TABLE brunn.web_sessions (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES brunn.users(id),
  credential_id uuid NOT NULL,
  token_hash brunn.sha256_hex NOT NULL UNIQUE,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  expires_at timestamptz NOT NULL,
  revoked_at timestamptz,
  FOREIGN KEY (user_id, credential_id)
    REFERENCES brunn.api_credentials(user_id, id),
  CHECK (expires_at > created_at),
  CHECK (expires_at <= created_at + interval '12 hours 1 minute'),
  CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);

CREATE INDEX web_sessions_active_idx
  ON brunn.web_sessions (user_id, expires_at)
  WHERE revoked_at IS NULL;

CREATE TABLE brunn.password_reset_tokens (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES brunn.users(id),
  token_hash brunn.sha256_hex NOT NULL UNIQUE,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  expires_at timestamptz NOT NULL,
  used_at timestamptz,
  CHECK (expires_at > created_at),
  CHECK (expires_at <= created_at + interval '31 minutes'),
  CHECK (used_at IS NULL OR used_at >= created_at)
);

CREATE INDEX password_reset_tokens_active_idx
  ON brunn.password_reset_tokens (user_id, expires_at)
  WHERE used_at IS NULL;

-- Identifier keys are HMAC-SHA-256 values generated with the continuation
-- secret. They bound brute force and reset-email amplification without storing
-- usernames or email addresses in the rate-limit ledger.
CREATE TABLE brunn.web_auth_rate_limits (
  kind text NOT NULL CHECK (kind IN ('login', 'reset')),
  identifier_hash brunn.sha256_hex NOT NULL,
  user_id uuid REFERENCES brunn.users(id),
  window_started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  attempts integer NOT NULL DEFAULT 1 CHECK (attempts > 0),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (kind, identifier_hash)
);

CREATE INDEX web_auth_rate_limits_updated_idx
  ON brunn.web_auth_rate_limits (updated_at);

-- Disabling the permanent Web UI principal must immediately invalidate every
-- browser session. This also covers operator recovery and account lifecycle
-- paths that disable credentials without going through the web-auth module.
CREATE FUNCTION brunn.revoke_web_sessions_on_credential_disable()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
BEGIN
  IF NEW.disabled_at IS NOT NULL AND OLD.disabled_at IS DISTINCT FROM NEW.disabled_at THEN
    UPDATE brunn.web_sessions
    SET revoked_at = coalesce(revoked_at, clock_timestamp())
    WHERE user_id = NEW.user_id
      AND credential_id = NEW.id
      AND revoked_at IS NULL;
    IF EXISTS (
      SELECT 1 FROM brunn.web_identities AS identity
      WHERE identity.user_id = NEW.user_id
        AND identity.web_credential_id = NEW.id
    ) THEN
      UPDATE brunn.password_reset_tokens
      SET used_at = coalesce(used_at, clock_timestamp())
      WHERE user_id = NEW.user_id AND used_at IS NULL;
    END IF;
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER api_credentials_revoke_web_sessions
AFTER UPDATE OF disabled_at ON brunn.api_credentials
FOR EACH ROW EXECUTE FUNCTION brunn.revoke_web_sessions_on_credential_disable();

-- Re-running operator identity configuration may change the sign-in address
-- or rebind its principal. Existing sessions must not survive that change.
CREATE FUNCTION brunn.revoke_web_sessions_on_identity_change()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
BEGIN
  UPDATE brunn.web_sessions
  SET revoked_at = coalesce(revoked_at, clock_timestamp())
  WHERE user_id = NEW.user_id AND revoked_at IS NULL;
  UPDATE brunn.password_reset_tokens
  SET used_at = coalesce(used_at, clock_timestamp())
  WHERE user_id = NEW.user_id AND used_at IS NULL;
  RETURN NEW;
END;
$$;

CREATE TRIGGER web_identities_revoke_web_sessions
AFTER UPDATE OF username, username_normalized, email, email_normalized,
                web_credential_id ON brunn.web_identities
FOR EACH ROW EXECUTE FUNCTION brunn.revoke_web_sessions_on_identity_change();

REVOKE ALL ON FUNCTION brunn.revoke_web_sessions_on_credential_disable() FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn.revoke_web_sessions_on_identity_change() FROM PUBLIC;

ALTER TABLE brunn.web_identities ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.web_identities FORCE ROW LEVEL SECURITY;
ALTER TABLE brunn.web_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.web_sessions FORCE ROW LEVEL SECURITY;
ALTER TABLE brunn.password_reset_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.password_reset_tokens FORCE ROW LEVEL SECURITY;
ALTER TABLE brunn.web_auth_rate_limits ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.web_auth_rate_limits FORCE ROW LEVEL SECURITY;

REVOKE ALL ON brunn.web_identities FROM app_rw, app_ro;
REVOKE ALL ON brunn.web_sessions FROM app_rw, app_ro;
REVOKE ALL ON brunn.password_reset_tokens FROM app_rw, app_ro;
REVOKE ALL ON brunn.web_auth_rate_limits FROM app_rw, app_ro;

CREATE FUNCTION brunn_auth.lookup_web_identity(p_identifier text)
RETURNS TABLE (
  user_id uuid,
  credential_id uuid,
  username text,
  email text,
  display_name text,
  password_hash text
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
  SELECT identity.user_id,
         identity.web_credential_id,
         identity.username::text,
         identity.email::text,
         user_row.display_name::text,
         identity.password_hash
  FROM brunn.web_identities AS identity
  JOIN brunn.users AS user_row ON user_row.id = identity.user_id
  JOIN brunn.api_credentials AS credential
    ON credential.user_id = identity.user_id
   AND credential.id = identity.web_credential_id
  WHERE user_row.account_status = 'active'
    AND credential.disabled_at IS NULL
    AND (
      identity.username_normalized = lower(btrim(p_identifier))
      OR identity.email_normalized = lower(btrim(p_identifier))
    )
  LIMIT 1
$$;

CREATE FUNCTION brunn_auth.consume_web_auth_rate_limit(
  p_kind text,
  p_identifier_hash text,
  p_user_id uuid DEFAULT NULL
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  attempt_limit integer;
  window_length interval;
  current_attempts integer;
BEGIN
  IF p_identifier_hash !~ '^[0-9a-f]{64}$' THEN
    RAISE EXCEPTION 'rate-limit identifier hash is invalid'
      USING ERRCODE = '22023';
  END IF;
  CASE p_kind
    WHEN 'login' THEN
      attempt_limit := 10;
      window_length := interval '15 minutes';
    WHEN 'reset' THEN
      attempt_limit := 5;
      window_length := interval '1 hour';
    ELSE
      RAISE EXCEPTION 'rate-limit kind is invalid' USING ERRCODE = '22023';
  END CASE;

  DELETE FROM brunn.web_auth_rate_limits
  WHERE updated_at < clock_timestamp() - interval '2 days';

  INSERT INTO brunn.web_auth_rate_limits AS bucket (
    kind, identifier_hash, user_id
  ) VALUES (
    p_kind, p_identifier_hash, p_user_id
  )
  ON CONFLICT (kind, identifier_hash) DO UPDATE
  SET user_id = coalesce(EXCLUDED.user_id, bucket.user_id),
      window_started_at = CASE
        WHEN bucket.window_started_at <= clock_timestamp() - window_length
          THEN clock_timestamp()
        ELSE bucket.window_started_at
      END,
      attempts = CASE
        WHEN bucket.window_started_at <= clock_timestamp() - window_length THEN 1
        ELSE bucket.attempts + 1
      END,
      updated_at = clock_timestamp()
  RETURNING attempts INTO current_attempts;

  RETURN current_attempts <= attempt_limit;
END;
$$;

CREATE FUNCTION brunn_auth.clear_web_auth_rate_limit(
  p_kind text,
  p_identifier_hash text
)
RETURNS void
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
  DELETE FROM brunn.web_auth_rate_limits
  WHERE kind = p_kind AND identifier_hash = p_identifier_hash
$$;

CREATE FUNCTION brunn_auth.create_web_session(
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
     OR p_expires_at > clock_timestamp() + interval '12 hours 1 minute' THEN
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
    AND identity.username_normalized = lower(btrim(p_expected_username))
  FOR UPDATE;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'verified password changed' USING ERRCODE = 'P0002';
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

CREATE FUNCTION brunn_auth.authenticate_web_session(p_token_hash text)
RETURNS TABLE (
  web_session_id uuid,
  credential_id uuid,
  user_id uuid,
  capabilities text[],
  scope_refs text[],
  expires_at timestamptz,
  username text,
  email text,
  display_name text
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
  SELECT session.id,
         credential.id,
         credential.user_id,
         credential.capabilities,
         coalesce(
           array_agg(scope.scope_ref::text ORDER BY scope.scope_ref)
             FILTER (WHERE scope.scope_ref IS NOT NULL),
           '{}'::text[]
         ),
         session.expires_at,
         identity.username::text,
         identity.email::text,
         user_row.display_name::text
  FROM brunn.web_sessions AS session
  JOIN brunn.web_identities AS identity
    ON identity.user_id = session.user_id
   AND identity.web_credential_id = session.credential_id
  JOIN brunn.users AS user_row ON user_row.id = session.user_id
  JOIN brunn.api_credentials AS credential
    ON credential.user_id = session.user_id
   AND credential.id = session.credential_id
  LEFT JOIN brunn.credential_scope_grants AS scope_grant
    ON scope_grant.user_id = credential.user_id
   AND scope_grant.credential_id = credential.id
  LEFT JOIN brunn.scopes AS scope
    ON scope.user_id = scope_grant.user_id
   AND scope.id = scope_grant.scope_id
  WHERE session.token_hash = p_token_hash
    AND session.revoked_at IS NULL
    AND session.expires_at > clock_timestamp()
    AND user_row.account_status = 'active'
    AND credential.disabled_at IS NULL
  GROUP BY session.id, credential.id, identity.username, identity.email,
           user_row.display_name
$$;

CREATE FUNCTION brunn_auth.revoke_web_session(p_token_hash text)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  revoked_user_id uuid;
  revoked_credential_id uuid;
  revoked_session_id uuid;
BEGIN
  UPDATE brunn.web_sessions AS session
  SET revoked_at = coalesce(session.revoked_at, clock_timestamp())
  WHERE session.token_hash = p_token_hash
    AND session.revoked_at IS NULL
  RETURNING session.user_id, session.credential_id, session.id
  INTO revoked_user_id, revoked_credential_id, revoked_session_id;

  IF revoked_session_id IS NULL THEN
    RETURN false;
  END IF;

  INSERT INTO brunn.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    revoked_user_id, revoked_credential_id, 'auth.web.logout',
    jsonb_build_object('web_session_id', revoked_session_id), true
  );
  RETURN true;
END;
$$;

CREATE FUNCTION brunn_auth.issue_password_reset(
  p_user_id uuid,
  p_token_hash text,
  p_expires_at timestamptz,
  p_expected_email text
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  reset_id uuid;
  principal_id uuid;
BEGIN
  IF p_token_hash !~ '^[0-9a-f]{64}$'
     OR p_expires_at <= clock_timestamp()
     OR p_expires_at > clock_timestamp() + interval '31 minutes' THEN
    RAISE EXCEPTION 'password reset parameters are invalid' USING ERRCODE = '22023';
  END IF;

  SELECT credential.id INTO principal_id
  FROM brunn.api_credentials AS credential
  JOIN brunn.web_identities AS identity
    ON credential.user_id = identity.user_id
   AND credential.id = identity.web_credential_id
  JOIN brunn.users AS user_row ON user_row.id = identity.user_id
  WHERE identity.user_id = p_user_id
    AND identity.email_normalized = lower(btrim(p_expected_email))
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
    AND identity.email_normalized = lower(btrim(p_expected_email))
  FOR UPDATE;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'web identity changed' USING ERRCODE = 'P0002';
  END IF;

  INSERT INTO brunn.password_reset_tokens (
    user_id, token_hash, expires_at
  ) VALUES (
    p_user_id, p_token_hash, p_expires_at
  ) RETURNING id INTO reset_id;

  INSERT INTO brunn.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id, principal_id, 'auth.password_reset.request',
    jsonb_build_object('password_reset_id', reset_id), true
  );
  RETURN reset_id;
END;
$$;

CREATE FUNCTION brunn_auth.consume_password_reset(
  p_token_hash text,
  p_password_hash text
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  reset_row brunn.password_reset_tokens%ROWTYPE;
  reset_user_id uuid;
  principal_id uuid;
BEGIN
  IF p_token_hash !~ '^[0-9a-f]{64}$'
     OR p_password_hash NOT LIKE '$argon2id$%' THEN
    RAISE EXCEPTION 'password reset material is invalid' USING ERRCODE = '22023';
  END IF;

  SELECT token.user_id INTO reset_user_id
  FROM brunn.password_reset_tokens AS token
  WHERE token.token_hash = p_token_hash;
  IF reset_user_id IS NULL THEN
    RAISE EXCEPTION 'password reset token is invalid or expired'
      USING ERRCODE = 'P0002';
  END IF;

  SELECT credential.id INTO principal_id
  FROM brunn.api_credentials AS credential
  JOIN brunn.web_identities AS identity
    ON credential.user_id = identity.user_id
   AND credential.id = identity.web_credential_id
  JOIN brunn.users AS user_row ON user_row.id = identity.user_id
  WHERE identity.user_id = reset_user_id
    AND user_row.account_status = 'active'
    AND credential.disabled_at IS NULL
  FOR UPDATE OF credential;
  IF principal_id IS NULL THEN
    RAISE EXCEPTION 'password reset token is invalid or expired'
      USING ERRCODE = 'P0002';
  END IF;

  PERFORM 1
  FROM brunn.web_identities AS identity
  WHERE identity.user_id = reset_user_id
    AND identity.web_credential_id = principal_id
  FOR UPDATE;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'password reset token is invalid or expired'
      USING ERRCODE = 'P0002';
  END IF;

  SELECT token.* INTO reset_row
  FROM brunn.password_reset_tokens AS token
  WHERE token.token_hash = p_token_hash
    AND token.user_id = reset_user_id
    AND token.used_at IS NULL
    AND token.expires_at > clock_timestamp()
  FOR UPDATE;

  IF reset_row.id IS NULL THEN
    RAISE EXCEPTION 'password reset token is invalid or expired'
      USING ERRCODE = 'P0002';
  END IF;

  UPDATE brunn.web_identities
  SET password_hash = p_password_hash,
      updated_at = clock_timestamp()
  WHERE user_id = reset_row.user_id
    AND web_credential_id = principal_id;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'web identity not found' USING ERRCODE = 'P0002';
  END IF;

  UPDATE brunn.password_reset_tokens
  SET used_at = coalesce(used_at, clock_timestamp())
  WHERE user_id = reset_row.user_id AND used_at IS NULL;
  UPDATE brunn.web_sessions
  SET revoked_at = coalesce(revoked_at, clock_timestamp())
  WHERE user_id = reset_row.user_id AND revoked_at IS NULL;
  DELETE FROM brunn.web_auth_rate_limits
  WHERE user_id = reset_row.user_id;

  INSERT INTO brunn.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    reset_row.user_id, principal_id, 'auth.password_reset.complete',
    jsonb_build_object('password_reset_id', reset_row.id), true
  );
  RETURN reset_row.user_id;
END;
$$;

-- The permanent Web UI principal has no usable bearer secret and must not be
-- listed or revoked through ordinary credential-management endpoints.
CREATE OR REPLACE FUNCTION brunn_auth.list_credentials(p_user_id uuid)
RETURNS TABLE (
  id uuid,
  label text,
  capabilities text[],
  scope_refs text[],
  created_at timestamptz,
  disabled_at timestamptz
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
BEGIN
  IF NOT brunn_auth.context_is_valid()
     OR brunn_auth.current_user_id() IS DISTINCT FROM p_user_id
     OR NOT brunn_auth.has_any_capability(ARRAY['status', 'read']) THEN
    RAISE EXCEPTION 'authenticated same-user status or read capability is required'
      USING ERRCODE = '42501';
  END IF;

  RETURN QUERY
  SELECT credential.id,
         credential.label::text,
         credential.capabilities,
         coalesce(
           array_agg(scope_row.scope_ref::text ORDER BY scope_row.scope_ref)
             FILTER (WHERE scope_row.id IS NOT NULL),
           '{}'::text[]
         ),
         credential.created_at,
         credential.disabled_at
  FROM brunn.api_credentials AS credential
  LEFT JOIN brunn.credential_scope_grants AS scope_grant
    ON scope_grant.user_id = credential.user_id
   AND scope_grant.credential_id = credential.id
  LEFT JOIN brunn.scopes AS scope_row
    ON scope_row.user_id = scope_grant.user_id
   AND scope_row.id = scope_grant.scope_id
  WHERE credential.user_id = p_user_id
    AND NOT EXISTS (
      SELECT 1 FROM brunn.web_identities AS identity
      WHERE identity.user_id = credential.user_id
        AND identity.web_credential_id = credential.id
    )
  GROUP BY credential.id, credential.label, credential.capabilities,
           credential.created_at, credential.disabled_at
  ORDER BY credential.created_at, credential.id;
END;
$$;

CREATE OR REPLACE FUNCTION brunn_auth.revoke_credential(
  p_user_id uuid,
  p_credential_id uuid
)
RETURNS timestamptz
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  revoked_at timestamptz;
BEGIN
  PERFORM brunn_auth.require_credential_control(p_user_id);

  UPDATE brunn.api_credentials AS credential
  SET disabled_at = coalesce(credential.disabled_at, clock_timestamp())
  WHERE credential.user_id = p_user_id
    AND credential.id = p_credential_id
    AND NOT EXISTS (
      SELECT 1 FROM brunn.web_identities AS identity
      WHERE identity.user_id = credential.user_id
        AND identity.web_credential_id = credential.id
    )
  RETURNING credential.disabled_at INTO revoked_at;

  IF revoked_at IS NULL THEN
    RAISE EXCEPTION 'credential not found for user' USING ERRCODE = 'P0002';
  END IF;

  INSERT INTO brunn.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id, NULL, brunn_auth.current_credential_id(),
    'auth.credential.revoke',
    jsonb_build_object('credential_id', p_credential_id, 'revoked_at', revoked_at),
    true
  );
  RETURN revoked_at;
END;
$$;

REVOKE ALL ON FUNCTION brunn_auth.lookup_web_identity(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.consume_web_auth_rate_limit(text, text, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.clear_web_auth_rate_limit(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.create_web_session(uuid, text, timestamptz, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.authenticate_web_session(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.revoke_web_session(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.issue_password_reset(uuid, text, timestamptz, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.consume_password_reset(text, text) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION brunn_auth.lookup_web_identity(text) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.consume_web_auth_rate_limit(text, text, uuid) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.clear_web_auth_rate_limit(text, text) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.create_web_session(uuid, text, timestamptz, text, text) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.authenticate_web_session(text) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.revoke_web_session(text) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.issue_password_reset(uuid, text, timestamptz, text) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.consume_password_reset(text, text) TO app_rw;

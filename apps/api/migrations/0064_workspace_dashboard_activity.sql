-- Product activity is an eventually consistent, content-free dashboard rollup.
-- The API records successful artifact reads and committed artifact mutations in
-- UTC minute buckets through a dedicated bounded fail-open tracker. Canonical
-- entry_usage events use a separate queue and worker, so dashboard load cannot
-- consume their buffer. Ordinary API roles may inspect their own account's
-- rollups, but only the admin pool writes them so telemetry can never alter a
-- foreground response.

CREATE TABLE straylight.product_activity_minutely (
  user_id uuid NOT NULL,
  credential_id uuid NOT NULL,
  bucket_start timestamptz NOT NULL,
  operation text NOT NULL CHECK (operation IN (
    'open', 'search', 'read', 'binary_fetch',
    'briefing_list', 'briefing_read', 'briefing_topics',
    'write', 'capture', 'checkpoint', 'binary_upload', 'delete',
    'briefing_publish', 'briefing_action'
  )),
  operation_count bigint NOT NULL DEFAULT 0 CHECK (operation_count >= 0),
  byte_count bigint NOT NULL DEFAULT 0 CHECK (byte_count >= 0),
  first_recorded_at timestamptz NOT NULL,
  last_recorded_at timestamptz NOT NULL,
  PRIMARY KEY (user_id, credential_id, bucket_start, operation),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id)
    ON DELETE CASCADE,
  CHECK (bucket_start = date_trunc('minute', bucket_start, 'UTC')),
  CHECK (first_recorded_at <= last_recorded_at),
  CHECK (
    first_recorded_at >= bucket_start
    AND last_recorded_at < bucket_start + interval '1 minute'
  )
);

CREATE INDEX product_activity_minutely_user_time_idx
  ON straylight.product_activity_minutely (
    user_id, bucket_start, credential_id, operation
  );

CREATE INDEX product_activity_minutely_credential_recent_idx
  ON straylight.product_activity_minutely (
    user_id, credential_id, last_recorded_at DESC, operation
  );

ALTER TABLE straylight.product_activity_minutely ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.product_activity_minutely FORCE ROW LEVEL SECURITY;

CREATE POLICY product_activity_minutely_select
  ON straylight.product_activity_minutely
  FOR SELECT TO app_rw, app_ro
  USING (
    straylight_auth.can_access_user(user_id)
    AND straylight_auth.has_capability('read')
    AND straylight_auth.has_capability('status')
  );

GRANT SELECT ON straylight.product_activity_minutely TO app_rw, app_ro;

-- Successful protected requests also update a content-free principal touch.
-- This is intentionally separate from product activity so control-plane reads
-- can update access visibility without inflating dashboard read/write totals.
CREATE TABLE straylight.credential_activity (
  user_id uuid NOT NULL,
  credential_id uuid NOT NULL,
  last_operation text NOT NULL CHECK (last_operation IN (
    'open', 'search', 'read', 'binary_fetch',
    'briefing_list', 'briefing_read', 'briefing_topics',
    'write', 'capture', 'checkpoint', 'binary_upload', 'delete',
    'briefing_publish', 'briefing_action',
    'dashboard', 'status', 'changes',
    'credential_list', 'credential_create', 'credential_update',
    'credential_delete', 'control'
  )),
  last_used_at timestamptz NOT NULL,
  request_count bigint NOT NULL DEFAULT 0 CHECK (request_count >= 0),
  PRIMARY KEY (user_id, credential_id),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id)
    ON DELETE CASCADE
);

CREATE INDEX credential_activity_user_recent_idx
  ON straylight.credential_activity (user_id, last_used_at DESC, credential_id);

ALTER TABLE straylight.credential_activity ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.credential_activity FORCE ROW LEVEL SECURITY;

-- Project the permanent Web UI principal as a synthetic, non-manageable row
-- alongside manageable API credentials. The underlying principal UUID is used
-- only for stable identity and activity joins; no token hash, password, browser
-- session, reset secret, or bearer value crosses this function.
CREATE FUNCTION straylight_auth.dashboard_credentials(p_user_id uuid)
RETURNS TABLE (
  id uuid,
  label text,
  kind text,
  manageable boolean,
  capabilities text[],
  scope_refs text[],
  created_at timestamptz,
  disabled_at timestamptz,
  last_used_at timestamptz,
  last_operation text
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
BEGIN
  IF NOT straylight_auth.context_is_valid()
     OR straylight_auth.current_user_id() IS DISTINCT FROM p_user_id
     OR NOT straylight_auth.has_capability('read')
     OR NOT straylight_auth.has_capability('status') THEN
    RAISE EXCEPTION 'authenticated same-user read and status capabilities are required'
      USING ERRCODE = '42501';
  END IF;

  RETURN QUERY
  WITH credential_scopes AS (
    SELECT credential.id,
           credential.user_id,
           CASE WHEN identity.web_credential_id IS NULL
             THEN credential.label::text
             ELSE 'Web UI'
           END AS label,
           CASE WHEN identity.web_credential_id IS NULL
             THEN 'api_credential'
             ELSE 'web_ui'
           END AS kind,
           identity.web_credential_id IS NULL AS manageable,
           credential.capabilities,
           credential.created_at,
           credential.disabled_at,
           coalesce(
             array_agg(scope_row.scope_ref::text ORDER BY scope_row.scope_ref)
               FILTER (WHERE scope_row.id IS NOT NULL),
             '{}'::text[]
           ) AS scope_refs
    FROM straylight.api_credentials AS credential
    LEFT JOIN straylight.web_identities AS identity
      ON identity.user_id = credential.user_id
     AND identity.web_credential_id = credential.id
    LEFT JOIN straylight.credential_scope_grants AS scope_grant
      ON scope_grant.user_id = credential.user_id
     AND scope_grant.credential_id = credential.id
    LEFT JOIN straylight.scopes AS scope_row
      ON scope_row.user_id = scope_grant.user_id
     AND scope_row.id = scope_grant.scope_id
    WHERE credential.user_id = p_user_id
    GROUP BY credential.id, credential.user_id, credential.label,
             identity.web_credential_id,
             credential.capabilities, credential.created_at,
             credential.disabled_at
  )
  SELECT credential.id,
         credential.label,
         credential.kind,
         credential.manageable,
         credential.capabilities,
         credential.scope_refs,
         credential.created_at,
         credential.disabled_at,
         activity.last_used_at,
         activity.last_operation
  FROM credential_scopes AS credential
  LEFT JOIN straylight.credential_activity AS activity
    ON activity.user_id = credential.user_id
   AND activity.credential_id = credential.id
  ORDER BY (credential.disabled_at IS NULL) DESC,
           activity.last_used_at DESC NULLS LAST,
           credential.manageable,
           credential.created_at DESC,
           credential.id;
END;
$$;

REVOKE ALL ON FUNCTION straylight_auth.dashboard_credentials(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight_auth.dashboard_credentials(uuid)
TO app_rw, app_ro;

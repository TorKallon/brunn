-- Product activity is an eventually consistent, content-free dashboard rollup.
-- The API records successful artifact reads and committed artifact mutations in
-- UTC hour buckets through the bounded fail-open usage tracker. Ordinary API
-- roles may inspect their own account's rollups, but only the admin pool writes
-- them so telemetry can never alter a foreground response.

CREATE TABLE straylight.product_activity_hourly (
  user_id uuid NOT NULL,
  credential_id uuid NOT NULL,
  bucket_start timestamptz NOT NULL,
  operation text NOT NULL CHECK (operation IN (
    'open', 'search', 'read', 'binary_fetch',
    'write', 'capture', 'checkpoint', 'binary_upload', 'delete'
  )),
  operation_count bigint NOT NULL DEFAULT 0 CHECK (operation_count >= 0),
  byte_count bigint NOT NULL DEFAULT 0 CHECK (byte_count >= 0),
  first_recorded_at timestamptz NOT NULL,
  last_recorded_at timestamptz NOT NULL,
  PRIMARY KEY (user_id, credential_id, bucket_start, operation),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id)
    ON DELETE CASCADE,
  CHECK (bucket_start = date_trunc('hour', bucket_start, 'UTC')),
  CHECK (first_recorded_at <= last_recorded_at),
  CHECK (
    first_recorded_at >= bucket_start
    AND last_recorded_at < bucket_start + interval '1 hour'
  )
);

CREATE INDEX product_activity_hourly_user_time_idx
  ON straylight.product_activity_hourly (
    user_id, bucket_start, credential_id, operation
  );

CREATE INDEX product_activity_hourly_credential_recent_idx
  ON straylight.product_activity_hourly (
    user_id, credential_id, last_recorded_at DESC, operation
  );

ALTER TABLE straylight.product_activity_hourly ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.product_activity_hourly FORCE ROW LEVEL SECURITY;

CREATE POLICY product_activity_hourly_select
  ON straylight.product_activity_hourly
  FOR SELECT TO app_rw, app_ro
  USING (
    straylight_auth.can_access_user(user_id)
    AND straylight_auth.has_capability('read')
    AND straylight_auth.has_capability('status')
  );

GRANT SELECT ON straylight.product_activity_hourly TO app_rw, app_ro;

-- Keep the permanent Web UI principal out of the API-token inventory, matching
-- list_credentials. No token hash or bearer value crosses this function.
CREATE FUNCTION straylight_auth.dashboard_credentials(p_user_id uuid)
RETURNS TABLE (
  id uuid,
  label text,
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
           credential.label::text AS label,
           credential.capabilities,
           credential.created_at,
           credential.disabled_at,
           coalesce(
             array_agg(scope_row.scope_ref::text ORDER BY scope_row.scope_ref)
               FILTER (WHERE scope_row.id IS NOT NULL),
             '{}'::text[]
           ) AS scope_refs
    FROM straylight.api_credentials AS credential
    LEFT JOIN straylight.credential_scope_grants AS scope_grant
      ON scope_grant.user_id = credential.user_id
     AND scope_grant.credential_id = credential.id
    LEFT JOIN straylight.scopes AS scope_row
      ON scope_row.user_id = scope_grant.user_id
     AND scope_row.id = scope_grant.scope_id
    WHERE credential.user_id = p_user_id
      AND NOT EXISTS (
        SELECT 1
        FROM straylight.web_identities AS identity
        WHERE identity.user_id = credential.user_id
          AND identity.web_credential_id = credential.id
      )
    GROUP BY credential.id, credential.user_id, credential.label,
             credential.capabilities, credential.created_at,
             credential.disabled_at
  )
  SELECT credential.id,
         credential.label,
         credential.capabilities,
         credential.scope_refs,
         credential.created_at,
         credential.disabled_at,
         recent.last_recorded_at,
         recent.operation
  FROM credential_scopes AS credential
  LEFT JOIN LATERAL (
    SELECT activity.last_recorded_at, activity.operation
    FROM straylight.product_activity_hourly AS activity
    WHERE activity.user_id = credential.user_id
      AND activity.credential_id = credential.id
    ORDER BY activity.last_recorded_at DESC,
             activity.bucket_start DESC,
             activity.operation
    LIMIT 1
  ) AS recent ON true
  ORDER BY (credential.disabled_at IS NULL) DESC,
           recent.last_recorded_at DESC NULLS LAST,
           credential.created_at DESC,
           credential.id;
END;
$$;

REVOKE ALL ON FUNCTION straylight_auth.dashboard_credentials(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight_auth.dashboard_credentials(uuid)
TO app_rw, app_ro;

-- Authenticate the complete RLS context once per transaction, then let row
-- policies compare only transaction-local user and scope identifiers. The
-- previous policy path re-ran credential and grant joins for every candidate
-- row, making the first read for an isolated user pathologically expensive.

CREATE FUNCTION straylight_auth.validate_transaction_context(
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
    WHERE api.id = p_credential_id
      AND api.user_id = p_user_id
      AND api.disabled_at IS NULL
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

CREATE FUNCTION straylight_auth.current_scope_ids()
RETURNS uuid[]
LANGUAGE sql
STABLE
AS $$
  SELECT coalesce(array_agg(item::uuid), '{}'::uuid[])
  FROM unnest(straylight_auth.setting_list('app.scope_ids')) AS item
$$;

CREATE OR REPLACE FUNCTION straylight_auth.can_access_scope(
  p_user_id uuid,
  p_scope_id uuid
)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY INVOKER
AS $$
  SELECT p_user_id = straylight_auth.current_user_id()
    AND straylight_auth.current_credential_id() IS NOT NULL
    AND p_scope_id = ANY(straylight_auth.current_scope_ids())
$$;

REVOKE ALL ON FUNCTION straylight_auth.validate_transaction_context(uuid, uuid, text[], text[]) FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.current_scope_ids() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight_auth.validate_transaction_context(uuid, uuid, text[], text[]) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION straylight_auth.current_scope_ids() TO app_rw, app_ro;

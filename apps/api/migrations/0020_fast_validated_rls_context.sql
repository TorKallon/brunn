-- The API validates credential state, requested capabilities, and scope grants
-- once before setting this transaction-local context. Keep row policies as a
-- defense-in-depth user/scope boundary without reparsing arrays or repeating
-- credential joins for every candidate row.

CREATE OR REPLACE FUNCTION straylight_auth.setting_uuid(p_name text)
RETURNS uuid
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
  SELECT nullif(current_setting(p_name, true), '')::uuid
$$;

CREATE OR REPLACE FUNCTION straylight_auth.setting_list(p_name text)
RETURNS text[]
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
  SELECT CASE
    WHEN coalesce(current_setting(p_name, true), '') = '' THEN '{}'::text[]
    ELSE string_to_array(current_setting(p_name, true), ',')
  END
$$;

CREATE OR REPLACE FUNCTION straylight_auth.current_scope_ids()
RETURNS uuid[]
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
  SELECT CASE
    WHEN coalesce(current_setting('app.scope_ids', true), '') = '' THEN '{}'::uuid[]
    ELSE string_to_array(current_setting('app.scope_ids', true), ',')::uuid[]
  END
$$;

CREATE OR REPLACE FUNCTION straylight_auth.context_is_valid()
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY INVOKER
PARALLEL SAFE
AS $$
  SELECT current_setting('app.context_valid', true) = 'true'
    AND nullif(current_setting('app.current_user_id', true), '') IS NOT NULL
    AND nullif(current_setting('app.current_credential_id', true), '') IS NOT NULL
$$;

CREATE OR REPLACE FUNCTION straylight_auth.can_access_user(p_user_id uuid)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY INVOKER
PARALLEL SAFE
AS $$
  SELECT straylight_auth.context_is_valid()
    AND p_user_id::text = current_setting('app.current_user_id', true)
$$;

CREATE OR REPLACE FUNCTION straylight_auth.can_access_scope(
  p_user_id uuid,
  p_scope_id uuid
)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY INVOKER
PARALLEL SAFE
AS $$
  SELECT straylight_auth.context_is_valid()
    AND p_user_id::text = current_setting('app.current_user_id', true)
    AND p_scope_id::text = ANY(straylight_auth.setting_list('app.scope_ids'))
$$;

REVOKE ALL ON FUNCTION straylight_auth.setting_uuid(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.setting_list(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.current_scope_ids() FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.context_is_valid() FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.can_access_user(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.can_access_scope(uuid, uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight_auth.setting_uuid(text) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION straylight_auth.setting_list(text) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION straylight_auth.current_scope_ids() TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION straylight_auth.context_is_valid() TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION straylight_auth.can_access_user(uuid) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION straylight_auth.can_access_scope(uuid, uuid) TO app_rw, app_ro;

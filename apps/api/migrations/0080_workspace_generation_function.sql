-- Migration 0080: deterministic workspace generation lookup.
-- The RLS quals on brunn.workspace_changes compare against
-- current_setting() values the planner cannot estimate, so
-- max(generation) flips between an index backward scan and an O(corpus)
-- sequential scan depending on statistics (the D09 64K tier measured the
-- open generation phase at ~64-95ms). Resolve the aggregate behind a
-- SECURITY DEFINER accessor pinned to the validated principal, matching
-- the search candidate functions; it returns a single bigint and never
-- exposes row data.

CREATE FUNCTION brunn_auth.workspace_generation(p_user_id uuid)
RETURNS bigint
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
BEGIN
  IF NOT brunn_auth.context_is_valid()
     OR brunn_auth.current_user_id() IS DISTINCT FROM p_user_id THEN
    RAISE EXCEPTION 'validated exact-principal context is required for generation lookup'
      USING ERRCODE = '42501';
  END IF;
  RETURN coalesce((
    SELECT max(generation)
    FROM brunn.workspace_changes
    WHERE user_id = p_user_id
  ), 0);
END;
$$;

REVOKE ALL ON FUNCTION brunn_auth.workspace_generation(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION brunn_auth.workspace_generation(uuid) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.workspace_generation(uuid) TO app_ro;

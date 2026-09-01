-- The application role may read account exports but cannot update arbitrary
-- rows. This narrow definer function acquires the row lock needed to keep an
-- object purge and the existing deletion transition serialized.

CREATE FUNCTION brunn_auth.lock_account_export_for_delete(
  p_user_id uuid,
  p_export_id uuid
)
RETURNS TABLE(status text, object_key text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
BEGIN
  PERFORM brunn_auth.require_credential_control(p_user_id);

  RETURN QUERY
  SELECT account_export.status,account_export.object_key
  FROM brunn.account_exports AS account_export
  WHERE account_export.user_id=p_user_id
    AND account_export.id=p_export_id
  FOR UPDATE;

  IF NOT FOUND THEN
    RAISE EXCEPTION 'account export not found' USING ERRCODE='P0002';
  END IF;
END;
$$;

REVOKE ALL ON FUNCTION brunn_auth.lock_account_export_for_delete(uuid,uuid)
FROM PUBLIC,app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.lock_account_export_for_delete(uuid,uuid)
TO app_rw;

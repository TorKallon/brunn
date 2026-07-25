-- Make account-export deletion recoverable across a process crash. PostgreSQL
-- first commits a durable deleting state while retaining the exact object
-- locator; the object is purged second; only then is the locator redacted.

ALTER TABLE straylight.account_exports
  DROP CONSTRAINT account_exports_status_check,
  ADD CONSTRAINT account_exports_status_check CHECK (status IN (
    'queued', 'running', 'ready', 'deleting', 'failed', 'expired', 'deleted'
  ));

DROP INDEX straylight.account_exports_one_active_idx;
CREATE UNIQUE INDEX account_exports_one_active_idx
  ON straylight.account_exports (user_id)
  WHERE status IN ('queued', 'running', 'ready', 'deleting');

CREATE FUNCTION straylight_auth.begin_account_export_delete(
  p_user_id uuid,
  p_export_id uuid
)
RETURNS TABLE(status text, object_key text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  current_status text;
  current_object_key text;
BEGIN
  PERFORM straylight_auth.require_credential_control(p_user_id);

  SELECT account_export.status,account_export.object_key
  INTO current_status,current_object_key
  FROM straylight.account_exports AS account_export
  WHERE account_export.user_id=p_user_id
    AND account_export.id=p_export_id
  FOR UPDATE;
  IF current_status IS NULL THEN
    RAISE EXCEPTION 'account export not found' USING ERRCODE = 'P0002';
  END IF;
  IF current_status = 'running' THEN
    RAISE EXCEPTION 'a running export cannot be deleted'
      USING ERRCODE = '55000';
  END IF;

  IF current_status <> 'deleted' AND current_object_key IS NOT NULL THEN
    UPDATE straylight.account_exports
    SET status='deleting',failure_code=NULL
    WHERE user_id=p_user_id AND id=p_export_id;
    current_status := 'deleting';
  ELSIF current_status <> 'deleted' THEN
    UPDATE straylight.account_exports
    SET status='deleted',object_key=NULL,object_version_id=NULL,
        completed_at=coalesce(completed_at,clock_timestamp()),
        failure_code=NULL
    WHERE user_id=p_user_id AND id=p_export_id;
    current_status := 'deleted';
  END IF;

  INSERT INTO straylight.audit_events (
    user_id,credential_id,action,details,content_free
  ) VALUES (
    p_user_id,
    straylight_auth.current_credential_id(),
    'account.export.delete.begin',
    jsonb_build_object(
      'export_id',p_export_id,
      'status',current_status,
      'object_purge_required',current_object_key IS NOT NULL
    ),
    true
  );

  RETURN QUERY SELECT current_status,current_object_key;
END;
$$;

CREATE FUNCTION straylight_auth.finish_account_export_delete(
  p_user_id uuid,
  p_export_id uuid
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  current_status text;
BEGIN
  PERFORM straylight_auth.require_credential_control(p_user_id);

  SELECT account_export.status
  INTO current_status
  FROM straylight.account_exports AS account_export
  WHERE account_export.user_id=p_user_id
    AND account_export.id=p_export_id
  FOR UPDATE;
  IF current_status IS NULL THEN
    RAISE EXCEPTION 'account export not found' USING ERRCODE = 'P0002';
  END IF;
  IF current_status NOT IN ('deleting','deleted') THEN
    RAISE EXCEPTION 'account export deletion was not durably started'
      USING ERRCODE = '55000';
  END IF;

  UPDATE straylight.account_exports
  SET status='deleted',object_key=NULL,object_version_id=NULL,
      completed_at=coalesce(completed_at,clock_timestamp()),
      failure_code=NULL
  WHERE user_id=p_user_id AND id=p_export_id;

  INSERT INTO straylight.audit_events (
    user_id,credential_id,action,details,content_free
  ) VALUES (
    p_user_id,
    straylight_auth.current_credential_id(),
    'account.export.delete.complete',
    jsonb_build_object('export_id',p_export_id),
    true
  );
END;
$$;

REVOKE ALL ON FUNCTION straylight_auth.begin_account_export_delete(uuid,uuid)
FROM PUBLIC,app_rw,app_ro;
REVOKE ALL ON FUNCTION straylight_auth.finish_account_export_delete(uuid,uuid)
FROM PUBLIC,app_rw,app_ro;
GRANT EXECUTE ON FUNCTION straylight_auth.begin_account_export_delete(uuid,uuid)
TO app_rw;
GRANT EXECUTE ON FUNCTION straylight_auth.finish_account_export_delete(uuid,uuid)
TO app_rw;

DROP FUNCTION straylight_auth.lock_account_export_for_delete(uuid,uuid);
DROP FUNCTION straylight_auth.mark_account_export_deleted(uuid,uuid);

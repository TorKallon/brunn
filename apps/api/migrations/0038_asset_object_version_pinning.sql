-- Provider object-version IDs make an immutable asset version name one exact
-- versioned object, rather than whichever bytes happen to be latest at its key.
-- NULL is retained for rows written before this migration and for providers
-- that do not return a version ID.

ALTER TABLE brunn.asset_versions
  ADD COLUMN object_version_id text,
  ADD CONSTRAINT asset_versions_object_version_id_nonempty CHECK (
    object_version_id IS NULL OR length(object_version_id) > 0
  );

ALTER TABLE brunn.asset_uploads
  ADD COLUMN temporary_object_version_id text,
  ADD COLUMN canonical_object_version_id text,
  ADD CONSTRAINT asset_uploads_temporary_object_version_id_nonempty CHECK (
    temporary_object_version_id IS NULL
    OR length(temporary_object_version_id) > 0
  ),
  ADD CONSTRAINT asset_uploads_canonical_object_version_id_nonempty CHECK (
    canonical_object_version_id IS NULL
    OR length(canonical_object_version_id) > 0
  ),
  ADD CONSTRAINT asset_uploads_canonical_object_version_has_key CHECK (
    canonical_object_version_id IS NULL OR canonical_object_key IS NOT NULL
  );

ALTER TABLE brunn.account_exports
  ADD COLUMN object_version_id text,
  ADD CONSTRAINT account_exports_object_version_id_nonempty CHECK (
    object_version_id IS NULL OR length(object_version_id) > 0
  ),
  ADD CONSTRAINT account_exports_object_version_has_key CHECK (
    object_version_id IS NULL OR object_key IS NOT NULL
  );

COMMENT ON COLUMN brunn.asset_versions.object_version_id IS
  'Opaque provider version ID for the exact immutable object; legacy NULL rows read the latest key.';
COMMENT ON COLUMN brunn.asset_uploads.temporary_object_version_id IS
  'Opaque provider version ID returned for the completed multipart source object.';
COMMENT ON COLUMN brunn.asset_uploads.canonical_object_version_id IS
  'Opaque provider version ID for the promoted content-addressed object.';
COMMENT ON COLUMN brunn.account_exports.object_version_id IS
  'Opaque provider version ID for the generated account-export archive.';

CREATE OR REPLACE FUNCTION brunn_auth.mark_account_export_deleted(
  p_user_id uuid,
  p_export_id uuid
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  current_status text;
BEGIN
  PERFORM brunn_auth.require_credential_control(p_user_id);

  SELECT status INTO current_status
  FROM brunn.account_exports
  WHERE user_id = p_user_id AND id = p_export_id
  FOR UPDATE;
  IF current_status IS NULL THEN
    RAISE EXCEPTION 'account export not found' USING ERRCODE = 'P0002';
  END IF;
  IF current_status = 'running' THEN
    RAISE EXCEPTION 'a running export cannot be deleted'
      USING ERRCODE = '55000';
  END IF;

  UPDATE brunn.account_exports
  SET status = 'deleted',
      object_key = NULL,
      object_version_id = NULL,
      completed_at = coalesce(completed_at, clock_timestamp())
  WHERE user_id = p_user_id AND id = p_export_id;

  INSERT INTO brunn.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    brunn_auth.current_credential_id(),
    'account.export.delete',
    jsonb_build_object('export_id', p_export_id),
    true
  );
END;
$$;

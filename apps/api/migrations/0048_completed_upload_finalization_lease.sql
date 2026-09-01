ALTER TABLE brunn.asset_uploads
  DROP CONSTRAINT asset_upload_completion_lease_consistent;

ALTER TABLE brunn.asset_uploads
  ADD CONSTRAINT asset_upload_completion_lease_consistent CHECK (
    (completion_token IS NULL) = (completion_lease_expires_at IS NULL)
    AND (
      completion_token IS NULL
      OR status IN ('verifying','completed')
    )
    AND (
      status <> 'verifying'
      OR completion_token IS NOT NULL
    )
  );

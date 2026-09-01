ALTER TABLE brunn.asset_uploads
  ADD COLUMN completion_token uuid,
  ADD COLUMN completion_lease_expires_at timestamptz,
  ADD COLUMN temporary_cleaned_at timestamptz;

ALTER TABLE brunn.asset_uploads
  ADD CONSTRAINT asset_upload_completion_lease_consistent CHECK (
    (
      status = 'verifying'
      AND completion_token IS NOT NULL
      AND completion_lease_expires_at IS NOT NULL
    )
    OR (
      status <> 'verifying'
      AND completion_token IS NULL
      AND completion_lease_expires_at IS NULL
    )
  );

CREATE INDEX asset_uploads_temporary_cleanup_idx
  ON brunn.asset_uploads (user_id, status, id)
  WHERE temporary_cleaned_at IS NULL;

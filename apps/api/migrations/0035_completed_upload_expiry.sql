DROP INDEX straylight.asset_uploads_expiry_idx;

CREATE INDEX asset_uploads_expiry_idx
  ON straylight.asset_uploads (status, expires_at, id)
  WHERE status IN ('uploading', 'verifying', 'completed');

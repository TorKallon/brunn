-- Physical blobs are content-addressed and may be shared by logical assets in
-- multiple scopes. Logical asset records themselves remain scope-local.
ALTER TABLE brunn.asset_versions
  DROP CONSTRAINT asset_versions_user_id_bucket_object_key_version_key;

CREATE INDEX asset_versions_object_lookup_idx
  ON brunn.asset_versions (user_id, bucket, object_key, version);

COMMENT ON INDEX brunn.asset_versions_object_lookup_idx IS
  'Supports user-wide physical blob deduplication without sharing scope-local logical asset records.';

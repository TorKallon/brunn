-- Operational state for authenticated, resumable native-asset uploads.
-- These tables do not add a new knowledge-domain concept: completed uploads
-- still enter the existing stage, asset-version, source, and corpus pipeline.

CREATE TABLE straylight.asset_uploads (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  credential_id uuid NOT NULL,
  stable_import_id text,
  path straylight.nonempty_text NOT NULL,
  media_type text NOT NULL,
  import_metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  describe_binary boolean NOT NULL DEFAULT true,
  expected_content_hash straylight.sha256_hex NOT NULL,
  expected_size_bytes bigint NOT NULL CHECK (
    expected_size_bytes > 0
    AND expected_size_bytes <= 5368709120
  ),
  part_size_bytes integer NOT NULL CHECK (
    part_size_bytes >= 5242880
    AND part_size_bytes <= 67108864
  ),
  temporary_object_key text NOT NULL,
  canonical_object_key text,
  multipart_upload_id text NOT NULL,
  stage_id uuid,
  status text NOT NULL CHECK (status IN (
    'uploading', 'verifying', 'completed', 'consumed',
    'aborted', 'failed', 'expired'
  )),
  failure_code text,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  completed_at timestamptz,
  consumed_at timestamptz,
  expires_at timestamptz NOT NULL,
  UNIQUE (user_id, id),
  UNIQUE (user_id, temporary_object_key),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  FOREIGN KEY (user_id, stage_id)
    REFERENCES straylight.stages(user_id, id),
  CHECK (path !~ '(^|/)\.\.(/|$)' AND left(path, 1) <> '/'),
  CHECK (length(path) <= 4096),
  CHECK (length(media_type) <= 255),
  CHECK (stable_import_id IS NULL OR length(stable_import_id) <= 240),
  CHECK (expires_at > created_at),
  CHECK (
    (status = 'completed' AND completed_at IS NOT NULL)
    OR status <> 'completed'
  ),
  CHECK (
    (status = 'consumed' AND completed_at IS NOT NULL AND consumed_at IS NOT NULL)
    OR status <> 'consumed'
  ),
  CHECK (
    (status IN ('completed', 'consumed') AND canonical_object_key IS NOT NULL)
    OR status NOT IN ('completed', 'consumed')
  ),
  CHECK (
    (status = 'consumed' AND stage_id IS NOT NULL)
    OR status <> 'consumed'
  )
);

CREATE INDEX asset_uploads_expiry_idx
  ON straylight.asset_uploads (status, expires_at, id)
  WHERE status IN ('uploading', 'verifying');

CREATE INDEX asset_uploads_scope_status_idx
  ON straylight.asset_uploads (user_id, scope_id, status, created_at);

CREATE INDEX source_episodes_stable_import_idx
  ON straylight.source_episodes (
    user_id,
    scope_id,
    (metadata->>'stable_import_id'),
    captured_at,
    id
  )
  WHERE metadata ? 'stable_import_id';

CREATE TABLE straylight.asset_upload_parts (
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  upload_id uuid NOT NULL,
  part_number integer NOT NULL CHECK (
    part_number >= 1 AND part_number <= 10000
  ),
  size_bytes integer NOT NULL CHECK (
    size_bytes > 0 AND size_bytes <= 67108864
  ),
  content_hash straylight.sha256_hex NOT NULL,
  etag text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, upload_id, part_number),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, upload_id)
    REFERENCES straylight.asset_uploads(user_id, id)
    ON DELETE CASCADE
);

ALTER TABLE straylight.asset_uploads ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.asset_uploads FORCE ROW LEVEL SECURITY;
ALTER TABLE straylight.asset_upload_parts ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.asset_upload_parts FORCE ROW LEVEL SECURITY;

CREATE POLICY asset_uploads_select
  ON straylight.asset_uploads
  FOR SELECT TO app_rw, app_ro
  USING (straylight_auth.can_access_scope(user_id, scope_id));

CREATE POLICY asset_uploads_write
  ON straylight.asset_uploads
  FOR ALL TO app_rw
  USING (
    straylight_auth.can_access_scope(user_id, scope_id)
    AND straylight_auth.has_any_capability(ARRAY['stage'])
  )
  WITH CHECK (
    straylight_auth.can_access_scope(user_id, scope_id)
    AND straylight_auth.has_any_capability(ARRAY['stage'])
  );

CREATE POLICY asset_upload_parts_select
  ON straylight.asset_upload_parts
  FOR SELECT TO app_rw, app_ro
  USING (straylight_auth.can_access_scope(user_id, scope_id));

CREATE POLICY asset_upload_parts_write
  ON straylight.asset_upload_parts
  FOR ALL TO app_rw
  USING (
    straylight_auth.can_access_scope(user_id, scope_id)
    AND straylight_auth.has_any_capability(ARRAY['stage'])
  )
  WITH CHECK (
    straylight_auth.can_access_scope(user_id, scope_id)
    AND straylight_auth.has_any_capability(ARRAY['stage'])
  );

GRANT SELECT, INSERT, UPDATE, DELETE
  ON straylight.asset_uploads, straylight.asset_upload_parts TO app_rw;
GRANT SELECT
  ON straylight.asset_uploads, straylight.asset_upload_parts TO app_ro;

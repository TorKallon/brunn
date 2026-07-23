CREATE TABLE straylight.assets (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  current_version integer NOT NULL DEFAULT 1 CHECK (current_version > 0),
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version)
);

CREATE TRIGGER assets_register_record
BEFORE INSERT ON straylight.assets
FOR EACH ROW EXECUTE FUNCTION straylight.register_record_key('asset');

CREATE TABLE straylight.asset_versions (
  user_id uuid NOT NULL,
  asset_id uuid NOT NULL,
  version integer NOT NULL CHECK (version > 0),
  previous_version integer,
  bucket straylight.nonempty_text NOT NULL,
  object_key straylight.nonempty_text NOT NULL,
  content_hash straylight.sha256_hex NOT NULL,
  size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
  media_type straylight.nonempty_text NOT NULL,
  etag text,
  encryption jsonb NOT NULL DEFAULT '{}'::jsonb,
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  stored_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, asset_id, version),
  FOREIGN KEY (user_id, asset_id)
    REFERENCES straylight.assets(user_id, id),
  FOREIGN KEY (user_id, asset_id, previous_version)
    REFERENCES straylight.asset_versions(user_id, asset_id, version)
    DEFERRABLE INITIALLY DEFERRED,
  CHECK (
    (version = 1 AND previous_version IS NULL)
    OR (version > 1 AND previous_version = version - 1)
  ),
  CHECK (object_key LIKE user_id::text || '/%'),
  UNIQUE (user_id, bucket, object_key, version),
  UNIQUE (user_id, content_hash, asset_id, version)
);

ALTER TABLE straylight.assets
  ADD CONSTRAINT assets_current_version_fk
  FOREIGN KEY (user_id, id, current_version)
  REFERENCES straylight.asset_versions(user_id, asset_id, version)
  DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE straylight.source_episodes (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  source_ref straylight.nonempty_text NOT NULL,
  source_kind straylight.nonempty_text NOT NULL,
  source_version text,
  title text,
  captured_at timestamptz NOT NULL,
  content_hash straylight.sha256_hex,
  native_locator jsonb NOT NULL DEFAULT '{}'::jsonb,
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, source_ref, source_version),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version)
);

CREATE TRIGGER source_episodes_register_record
BEFORE INSERT ON straylight.source_episodes
FOR EACH ROW EXECUTE FUNCTION straylight.register_record_key('source_episode');

CREATE TABLE straylight.source_asset_links (
  user_id uuid NOT NULL,
  source_episode_id uuid NOT NULL,
  asset_id uuid NOT NULL,
  asset_version integer NOT NULL,
  role straylight.nonempty_text NOT NULL,
  PRIMARY KEY (user_id, source_episode_id, asset_id, asset_version, role),
  FOREIGN KEY (user_id, source_episode_id)
    REFERENCES straylight.source_episodes(user_id, id),
  FOREIGN KEY (user_id, asset_id, asset_version)
    REFERENCES straylight.asset_versions(user_id, asset_id, version)
);

CREATE TABLE straylight.evidence_items (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  source_episode_id uuid NOT NULL,
  evidence_kind straylight.nonempty_text NOT NULL,
  locator jsonb NOT NULL,
  text_content text,
  content_hash straylight.sha256_hex NOT NULL,
  observed_at timestamptz,
  asset_id uuid,
  asset_version integer,
  policy_id uuid NOT NULL,
  policy_version integer NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, source_episode_id)
    REFERENCES straylight.source_episodes(user_id, id),
  FOREIGN KEY (user_id, asset_id, asset_version)
    REFERENCES straylight.asset_versions(user_id, asset_id, version),
  FOREIGN KEY (user_id, policy_id, policy_version)
    REFERENCES straylight.policy_revisions(user_id, policy_id, version),
  CHECK ((asset_id IS NULL) = (asset_version IS NULL)),
  CHECK (text_content IS NOT NULL OR asset_id IS NOT NULL)
);

CREATE TRIGGER evidence_items_register_record
BEFORE INSERT ON straylight.evidence_items
FOR EACH ROW EXECUTE FUNCTION straylight.register_record_key('evidence');

CREATE INDEX evidence_source_idx
  ON straylight.evidence_items (user_id, scope_id, source_episode_id, created_at);

CREATE TRIGGER asset_versions_immutable
BEFORE UPDATE OR DELETE ON straylight.asset_versions
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER source_episodes_immutable
BEFORE UPDATE OR DELETE ON straylight.source_episodes
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER source_asset_links_immutable
BEFORE UPDATE OR DELETE ON straylight.source_asset_links
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER evidence_items_immutable
BEFORE UPDATE OR DELETE ON straylight.evidence_items
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER assets_head_advance
BEFORE UPDATE OF current_version ON straylight.assets
FOR EACH ROW EXECUTE FUNCTION straylight.enforce_sequential_head_advance();

CREATE TRIGGER assets_no_delete
BEFORE DELETE ON straylight.assets
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_physical_delete();


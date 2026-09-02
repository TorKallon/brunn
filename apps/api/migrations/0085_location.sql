-- Owner location evidence and deterministic derived presence.

ALTER TABLE brunn.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_capabilities_check2;

ALTER TABLE brunn.api_credentials
  ADD CONSTRAINT api_credentials_capabilities_check2 CHECK (
    capabilities <@ ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'notification:publish', 'notification:manage',
      'secret:read', 'secret:write',
      'task.read', 'task.write', 'location.write', 'integration.manage',
      'message.read', 'message.write',
      'admin'
    ]::text[]
  );

ALTER TABLE brunn.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_owner_full_capabilities_check;

UPDATE brunn.api_credentials
SET capabilities = capabilities || ARRAY['location.write']::text[]
WHERE capabilities @> ARRAY['save']::text[]
  AND NOT capabilities @> ARRAY['location.write']::text[];

ALTER TABLE brunn.api_credentials
  ADD CONSTRAINT api_credentials_owner_full_capabilities_check CHECK (
    NOT capabilities @> ARRAY['credential:manage']::text[]
    OR capabilities @> ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'notification:publish', 'notification:manage',
      'secret:read', 'secret:write',
      'task.read', 'task.write', 'location.write', 'integration.manage',
      'admin'
    ]::text[]
  );

CREATE OR REPLACE FUNCTION brunn_auth.admin_issue_credential(
  p_user_id uuid,
  p_label text,
  p_token_hash text,
  p_capabilities text[],
  p_scope_refs text[]
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  allowed_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'notification:publish', 'notification:manage',
    'secret:read', 'secret:write',
    'task.read', 'task.write', 'location.write', 'integration.manage',
    'message.read', 'message.write',
    'admin'
  ];
  created_credential_id uuid;
  matched_scope_count integer;
BEGIN
  PERFORM brunn_auth.require_admin();
  IF p_capabilities IS NULL
     OR cardinality(p_capabilities) = 0
     OR array_position(p_capabilities, NULL) IS NOT NULL
     OR NOT p_capabilities <@ allowed_capabilities
     OR cardinality(p_capabilities) <> (
       SELECT count(DISTINCT capability)
       FROM unnest(p_capabilities) AS capability
     ) THEN
    RAISE EXCEPTION 'recovery capabilities are invalid'
      USING ERRCODE = '22023';
  END IF;
  IF p_scope_refs IS NULL
     OR cardinality(p_scope_refs) = 0
     OR array_position(p_scope_refs, NULL) IS NOT NULL
     OR cardinality(p_scope_refs) <> (
       SELECT count(DISTINCT scope_ref)
       FROM unnest(p_scope_refs) AS scope_ref
     ) THEN
    RAISE EXCEPTION 'recovery scope_refs must be a nonempty set'
      USING ERRCODE = '22023';
  END IF;
  IF NOT EXISTS (
    SELECT 1 FROM brunn.users
    WHERE id = p_user_id AND account_status = 'active'
  ) THEN
    RAISE EXCEPTION 'active recovery user not found' USING ERRCODE = 'P0002';
  END IF;
  SELECT count(*) INTO matched_scope_count
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  IF matched_scope_count <> cardinality(p_scope_refs) THEN
    RAISE EXCEPTION 'one or more recovery scopes do not belong to the user'
      USING ERRCODE = '22023';
  END IF;
  INSERT INTO brunn.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (p_user_id, p_label, p_token_hash, p_capabilities)
  RETURNING id INTO created_credential_id;
  INSERT INTO brunn.credential_scope_grants (
    credential_id, user_id, scope_id
  )
  SELECT created_credential_id, p_user_id, scope_row.id
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  INSERT INTO brunn.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    brunn_auth.current_user_id(),
    brunn_auth.current_credential_id(),
    'admin.credential.recover',
    jsonb_build_object(
      'target_user_id', p_user_id,
      'credential_id', created_credential_id,
      'scope_refs', p_scope_refs
    ),
    true
  );
  RETURN created_credential_id;
END;
$$;

CREATE OR REPLACE FUNCTION brunn_auth.issue_credential(
  p_user_id uuid,
  p_label text,
  p_token_hash text,
  p_capabilities text[],
  p_scope_refs text[]
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  allowed_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'notification:publish', 'notification:manage',
    'secret:read', 'secret:write',
    'task.read', 'task.write', 'location.write', 'integration.manage',
    'message.read', 'message.write',
    'admin'
  ];
  created_credential_id uuid;
  matched_scope_count integer;
BEGIN
  PERFORM brunn_auth.require_credential_control(p_user_id);
  IF p_capabilities IS NULL
     OR cardinality(p_capabilities) = 0
     OR array_position(p_capabilities, NULL) IS NOT NULL
     OR NOT p_capabilities <@ allowed_capabilities
     OR NOT p_capabilities <@ brunn_auth.current_capabilities()
     OR cardinality(p_capabilities) <> (
       SELECT count(DISTINCT capability)
       FROM unnest(p_capabilities) AS capability
     ) THEN
    RAISE EXCEPTION 'capabilities exceed the caller authority or are invalid'
      USING ERRCODE = '42501';
  END IF;
  IF p_scope_refs IS NULL
     OR cardinality(p_scope_refs) = 0
     OR array_position(p_scope_refs, NULL) IS NOT NULL
     OR NOT p_scope_refs <@ brunn_auth.current_scope_refs()
     OR cardinality(p_scope_refs) <> (
       SELECT count(DISTINCT scope_ref)
       FROM unnest(p_scope_refs) AS scope_ref
     ) THEN
    RAISE EXCEPTION 'scope_refs exceed the caller authority or are invalid'
      USING ERRCODE = '42501';
  END IF;
  SELECT count(*) INTO matched_scope_count
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  IF matched_scope_count <> cardinality(p_scope_refs) THEN
    RAISE EXCEPTION 'one or more scope_refs do not belong to the user'
      USING ERRCODE = '22023';
  END IF;
  INSERT INTO brunn.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    p_user_id, p_label, p_token_hash, p_capabilities
  ) RETURNING id INTO created_credential_id;
  INSERT INTO brunn.credential_scope_grants (
    credential_id, user_id, scope_id
  )
  SELECT created_credential_id, p_user_id, scope_row.id
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  INSERT INTO brunn.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    NULL,
    brunn_auth.current_credential_id(),
    'auth.credential.issue',
    jsonb_build_object(
      'credential_id', created_credential_id,
      'capabilities', p_capabilities,
      'scope_refs', p_scope_refs
    ),
    true
  );
  RETURN created_credential_id;
END;
$$;

CREATE OR REPLACE FUNCTION brunn_auth.admin_provision_user(
  p_external_ref text,
  p_display_name text,
  p_credential_label text,
  p_token_hash text,
  p_empty_manifest_hash text
)
RETURNS TABLE(user_id uuid, credential_id uuid, scope_id uuid, policy_id uuid)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  owner_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'notification:publish', 'notification:manage',
    'secret:read', 'secret:write',
    'task.read', 'task.write', 'location.write', 'integration.manage',
    'message.read', 'message.write',
    'admin'
  ];
  created_user_id uuid;
  created_credential_id uuid;
  root_scope_id uuid;
  default_policy_id uuid;
BEGIN
  PERFORM brunn_auth.require_admin();
  IF p_external_ref IS NULL OR btrim(p_external_ref) = ''
     OR length(p_external_ref) > 200
     OR p_display_name IS NULL OR btrim(p_display_name) = ''
     OR length(p_display_name) > 200
     OR p_credential_label IS NULL OR btrim(p_credential_label) = ''
     OR length(p_credential_label) > 120 THEN
    RAISE EXCEPTION 'provisioning names are invalid' USING ERRCODE = '22023';
  END IF;
  IF EXISTS (
    SELECT 1 FROM brunn.users AS existing_user
    WHERE existing_user.external_ref = p_external_ref
  ) THEN
    RAISE EXCEPTION 'external_ref already exists' USING ERRCODE = '23505';
  END IF;
  INSERT INTO brunn.users (external_ref, display_name)
  VALUES (p_external_ref, p_display_name)
  RETURNING users.id INTO created_user_id;
  SELECT scope_row.id INTO root_scope_id
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = created_user_id
    AND scope_row.scope_ref = 'scope:root';
  SELECT policy_row.id INTO default_policy_id
  FROM brunn.policies AS policy_row
  WHERE policy_row.user_id = created_user_id
    AND policy_row.is_default;
  INSERT INTO brunn.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    created_user_id, p_credential_label, p_token_hash, owner_capabilities
  ) RETURNING api_credentials.id INTO created_credential_id;
  INSERT INTO brunn.credential_scope_grants (
    credential_id, user_id, scope_id
  ) VALUES (created_credential_id, created_user_id, root_scope_id);
  INSERT INTO brunn.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    brunn_auth.current_user_id(),
    brunn_auth.current_credential_id(),
    'admin.user.provision',
    jsonb_build_object('target_user_id', created_user_id),
    true
  );
  RETURN QUERY SELECT
    created_user_id, created_credential_id, root_scope_id,
    default_policy_id;
END;
$$;

CREATE TABLE brunn.location_reports (
  user_id uuid NOT NULL REFERENCES brunn.users(id) ON DELETE CASCADE,
  at timestamptz NOT NULL,
  type text NOT NULL,
  offset_min smallint NOT NULL,
  lat double precision NOT NULL,
  lon double precision NOT NULL,
  accuracy_m real NOT NULL,
  arrived_at timestamptz,
  departed_at timestamptz,
  city text,
  region text,
  country text,
  name text,
  PRIMARY KEY (user_id, at, type)
);

CREATE TABLE brunn.location_report_poi (
  user_id uuid NOT NULL,
  at timestamptz NOT NULL,
  type text NOT NULL,
  rank smallint NOT NULL,
  name text NOT NULL,
  category text,
  distance_m real NOT NULL,
  PRIMARY KEY (user_id, at, type, rank),
  FOREIGN KEY (user_id, at, type)
    REFERENCES brunn.location_reports(user_id, at, type) ON DELETE CASCADE
);

CREATE TABLE brunn.location_presence (
  user_id uuid PRIMARY KEY REFERENCES brunn.users(id) ON DELETE CASCADE,
  timezone text NOT NULL,
  reported_at timestamptz NOT NULL,
  last_lat double precision NOT NULL,
  last_lon double precision NOT NULL,
  last_accuracy_m real NOT NULL,
  city text,
  region text,
  country text,
  visit_arrived_at timestamptz,
  visit_lat double precision,
  visit_lon double precision,
  visit_label text,
  visit_kind text,
  visit_confidence text,
  CHECK (
    (visit_arrived_at IS NULL) = (visit_lat IS NULL)
    AND (visit_arrived_at IS NULL) = (visit_lon IS NULL)
    AND (visit_arrived_at IS NULL) = (visit_kind IS NULL)
    AND (visit_arrived_at IS NULL) = (visit_confidence IS NULL)
  )
);

ALTER TABLE brunn.location_reports ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.location_reports FORCE ROW LEVEL SECURITY;
ALTER TABLE brunn.location_report_poi ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.location_report_poi FORCE ROW LEVEL SECURITY;
ALTER TABLE brunn.location_presence ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.location_presence FORCE ROW LEVEL SECURITY;

CREATE POLICY location_reports_select
ON brunn.location_reports
FOR SELECT TO app_rw
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['save', 'admin'])
);

CREATE POLICY location_reports_insert
ON brunn.location_reports
FOR INSERT TO app_rw
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
);

CREATE POLICY location_reports_delete
ON brunn.location_reports
FOR DELETE TO app_rw
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
);

CREATE POLICY location_report_poi_select
ON brunn.location_report_poi
FOR SELECT TO app_rw
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['save', 'admin'])
);

CREATE POLICY location_report_poi_insert
ON brunn.location_report_poi
FOR INSERT TO app_rw
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
);

CREATE POLICY location_report_poi_delete
ON brunn.location_report_poi
FOR DELETE TO app_rw
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
);

CREATE POLICY location_presence_select
ON brunn.location_presence
FOR SELECT TO app_rw, app_ro
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(
    ARRAY['open', 'read', 'save', 'location.write', 'admin']
  )
);

CREATE POLICY location_presence_insert
ON brunn.location_presence
FOR INSERT TO app_rw
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['save', 'location.write', 'admin'])
);

CREATE POLICY location_presence_update
ON brunn.location_presence
FOR UPDATE TO app_rw
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['save', 'location.write', 'admin'])
)
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['save', 'location.write', 'admin'])
);

CREATE POLICY location_presence_delete
ON brunn.location_presence
FOR DELETE TO app_rw
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['save', 'location.write', 'admin'])
);

GRANT SELECT, INSERT, DELETE ON
  brunn.location_reports,
  brunn.location_report_poi
TO app_rw;

GRANT SELECT, INSERT, UPDATE, DELETE ON brunn.location_presence TO app_rw;
GRANT SELECT ON brunn.location_presence TO app_ro;

-- A location reporter may version only canonical derived month files through
-- the shared workspace writer. It does not gain the general save capability.
CREATE POLICY location_entries_insert
ON brunn.entries
FOR INSERT TO app_rw
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
  AND path ~ '^Location/Visits/[0-9]{4}-(0[1-9]|1[0-2])\.md$'
);

CREATE POLICY location_entries_update
ON brunn.entries
FOR UPDATE TO app_rw
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
  AND path ~ '^Location/Visits/[0-9]{4}-(0[1-9]|1[0-2])\.md$'
)
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
  AND path ~ '^Location/Visits/[0-9]{4}-(0[1-9]|1[0-2])\.md$'
);

CREATE POLICY location_entry_versions_insert
ON brunn.entry_versions
FOR INSERT TO app_rw
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
  AND EXISTS (
    SELECT 1
    FROM brunn.entries AS entry
    WHERE entry.user_id=entry_versions.user_id
      AND entry.id=entry_versions.entry_id
      AND entry.path ~ '^Location/Visits/[0-9]{4}-(0[1-9]|1[0-2])\.md$'
  )
);

CREATE POLICY location_workspace_changes_insert
ON brunn.workspace_changes
FOR INSERT TO app_rw
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
  AND path ~ '^Location/Visits/[0-9]{4}-(0[1-9]|1[0-2])\.md$'
  AND EXISTS (
    SELECT 1
    FROM brunn.entries AS entry
    WHERE entry.user_id=workspace_changes.user_id
      AND entry.id=workspace_changes.entry_id
      AND entry.path=workspace_changes.path
  )
);

CREATE POLICY location_search_chunks_insert
ON brunn.search_chunks
FOR INSERT TO app_rw
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
  AND path ~ '^Location/Visits/[0-9]{4}-(0[1-9]|1[0-2])\.md$'
  AND EXISTS (
    SELECT 1
    FROM brunn.entries AS entry
    JOIN brunn.entry_versions AS version
      ON version.user_id=entry.user_id
     AND version.entry_id=entry.id
    WHERE entry.user_id=search_chunks.user_id
      AND entry.id=search_chunks.entry_id
      AND entry.path=search_chunks.path
      AND version.id=search_chunks.entry_version_id
  )
);

CREATE POLICY location_search_chunks_delete
ON brunn.search_chunks
FOR DELETE TO app_rw
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
  AND path ~ '^Location/Visits/[0-9]{4}-(0[1-9]|1[0-2])\.md$'
  AND EXISTS (
    SELECT 1
    FROM brunn.entries AS entry
    JOIN brunn.entry_versions AS version
      ON version.user_id=entry.user_id
     AND version.entry_id=entry.id
    WHERE entry.user_id=search_chunks.user_id
      AND entry.id=search_chunks.entry_id
      AND entry.path=search_chunks.path
      AND version.id=search_chunks.entry_version_id
  )
);

CREATE POLICY location_jobs_insert
ON brunn.jobs
FOR INSERT TO app_rw
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['location.write', 'admin'])
  AND kind='embed_entry'
  AND EXISTS (
    SELECT 1
    FROM brunn.entries AS entry
    WHERE entry.user_id=jobs.user_id
      AND entry.path ~ '^Location/Visits/[0-9]{4}-(0[1-9]|1[0-2])\.md$'
      AND entry.deleted_at IS NULL
      AND jobs.payload=jsonb_build_object(
        'entry_id', entry.id,
        'version', entry.current_version
      )
  )
);

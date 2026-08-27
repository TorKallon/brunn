-- Agent-first tasks. Canonical task state remains in versioned workspace
-- entries; these tables are registries, immutable evidence, integration state,
-- and rebuildable projections.

ALTER TABLE straylight.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_capabilities_check2;

ALTER TABLE straylight.api_credentials
  ADD CONSTRAINT api_credentials_capabilities_check2 CHECK (
    capabilities <@ ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'notification:publish', 'notification:manage',
      'secret:read', 'secret:write',
      'task.read', 'task.write', 'integration.manage',
      'admin'
    ]::text[]
  );

ALTER TABLE straylight.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_owner_full_capabilities_check;

UPDATE straylight.api_credentials
SET capabilities = capabilities || ARRAY['task.read']::text[]
WHERE capabilities @> ARRAY['read']::text[]
  AND NOT capabilities @> ARRAY['task.read']::text[];

UPDATE straylight.api_credentials
SET capabilities = capabilities || ARRAY['task.write']::text[]
WHERE capabilities @> ARRAY['save']::text[]
  AND NOT capabilities @> ARRAY['task.write']::text[];

UPDATE straylight.api_credentials
SET capabilities = capabilities || ARRAY['integration.manage']::text[]
WHERE capabilities @> ARRAY['credential:manage']::text[]
  AND NOT capabilities @> ARRAY['integration.manage']::text[];

ALTER TABLE straylight.api_credentials
  ADD CONSTRAINT api_credentials_owner_full_capabilities_check CHECK (
    NOT capabilities @> ARRAY['credential:manage']::text[]
    OR capabilities @> ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'notification:publish', 'notification:manage',
      'secret:read', 'secret:write',
      'task.read', 'task.write', 'integration.manage',
      'admin'
    ]::text[]
  );

CREATE OR REPLACE FUNCTION straylight_auth.admin_issue_credential(
  p_user_id uuid,
  p_label text,
  p_token_hash text,
  p_capabilities text[],
  p_scope_refs text[]
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  allowed_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'notification:publish', 'notification:manage',
    'secret:read', 'secret:write',
    'task.read', 'task.write', 'integration.manage',
    'admin'
  ];
  created_credential_id uuid;
  matched_scope_count integer;
BEGIN
  PERFORM straylight_auth.require_admin();
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
    SELECT 1 FROM straylight.users
    WHERE id = p_user_id AND account_status = 'active'
  ) THEN
    RAISE EXCEPTION 'active recovery user not found' USING ERRCODE = 'P0002';
  END IF;
  SELECT count(*) INTO matched_scope_count
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  IF matched_scope_count <> cardinality(p_scope_refs) THEN
    RAISE EXCEPTION 'one or more recovery scopes do not belong to the user'
      USING ERRCODE = '22023';
  END IF;
  INSERT INTO straylight.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (p_user_id, p_label, p_token_hash, p_capabilities)
  RETURNING id INTO created_credential_id;
  INSERT INTO straylight.credential_scope_grants (
    credential_id, user_id, scope_id
  )
  SELECT created_credential_id, p_user_id, scope_row.id
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  INSERT INTO straylight.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    straylight_auth.current_user_id(),
    straylight_auth.current_credential_id(),
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

CREATE OR REPLACE FUNCTION straylight_auth.issue_credential(
  p_user_id uuid,
  p_label text,
  p_token_hash text,
  p_capabilities text[],
  p_scope_refs text[]
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  allowed_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'notification:publish', 'notification:manage',
    'secret:read', 'secret:write',
    'task.read', 'task.write', 'integration.manage',
    'admin'
  ];
  created_credential_id uuid;
  matched_scope_count integer;
BEGIN
  PERFORM straylight_auth.require_credential_control(p_user_id);
  IF p_capabilities IS NULL
     OR cardinality(p_capabilities) = 0
     OR array_position(p_capabilities, NULL) IS NOT NULL
     OR NOT p_capabilities <@ allowed_capabilities
     OR NOT p_capabilities <@ straylight_auth.current_capabilities()
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
     OR NOT p_scope_refs <@ straylight_auth.current_scope_refs()
     OR cardinality(p_scope_refs) <> (
       SELECT count(DISTINCT scope_ref)
       FROM unnest(p_scope_refs) AS scope_ref
     ) THEN
    RAISE EXCEPTION 'scope_refs exceed the caller authority or are invalid'
      USING ERRCODE = '42501';
  END IF;
  SELECT count(*) INTO matched_scope_count
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  IF matched_scope_count <> cardinality(p_scope_refs) THEN
    RAISE EXCEPTION 'one or more scope_refs do not belong to the user'
      USING ERRCODE = '22023';
  END IF;
  INSERT INTO straylight.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    p_user_id, p_label, p_token_hash, p_capabilities
  ) RETURNING id INTO created_credential_id;
  INSERT INTO straylight.credential_scope_grants (
    credential_id, user_id, scope_id
  )
  SELECT created_credential_id, p_user_id, scope_row.id
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);
  INSERT INTO straylight.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    NULL,
    straylight_auth.current_credential_id(),
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

CREATE OR REPLACE FUNCTION straylight_auth.admin_provision_user(
  p_external_ref text,
  p_display_name text,
  p_credential_label text,
  p_token_hash text,
  p_empty_manifest_hash text
)
RETURNS TABLE (
  user_id uuid,
  credential_id uuid,
  scope_id uuid,
  policy_id uuid,
  corpus_revision_id uuid
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  owner_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'notification:publish', 'notification:manage',
    'secret:read', 'secret:write',
    'task.read', 'task.write', 'integration.manage',
    'admin'
  ];
  created_user_id uuid;
  created_credential_id uuid;
  root_scope_id uuid;
  default_policy_id uuid;
  initial_revision_id uuid := gen_random_uuid();
  manifest_id uuid := gen_random_uuid();
BEGIN
  PERFORM straylight_auth.require_admin();
  IF p_external_ref IS NULL OR btrim(p_external_ref) = ''
     OR length(p_external_ref) > 200
     OR p_display_name IS NULL OR btrim(p_display_name) = ''
     OR length(p_display_name) > 200
     OR p_credential_label IS NULL OR btrim(p_credential_label) = ''
     OR length(p_credential_label) > 120 THEN
    RAISE EXCEPTION 'provisioning names are invalid' USING ERRCODE = '22023';
  END IF;
  IF EXISTS (
    SELECT 1 FROM straylight.users AS existing_user
    WHERE existing_user.external_ref = p_external_ref
  ) THEN
    RAISE EXCEPTION 'external_ref already exists' USING ERRCODE = '23505';
  END IF;
  INSERT INTO straylight.users (external_ref, display_name)
  VALUES (p_external_ref, p_display_name)
  RETURNING users.id INTO created_user_id;
  SELECT scope_row.id INTO root_scope_id
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = created_user_id
    AND scope_row.scope_ref = 'scope:root';
  SELECT policy_row.id INTO default_policy_id
  FROM straylight.policies AS policy_row
  WHERE policy_row.user_id = created_user_id
    AND policy_row.is_default;
  INSERT INTO straylight.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    created_user_id, p_credential_label, p_token_hash, owner_capabilities
  ) RETURNING api_credentials.id INTO created_credential_id;
  INSERT INTO straylight.credential_scope_grants (
    credential_id, user_id, scope_id
  ) VALUES (created_credential_id, created_user_id, root_scope_id);
  INSERT INTO straylight.corpus_revisions (
    id, user_id, scope_id, parent_revision_id, revision_number, manifest_hash
  ) VALUES (
    initial_revision_id, created_user_id, root_scope_id, NULL, 1, p_empty_manifest_hash
  );
  INSERT INTO straylight.active_manifests (
    id, user_id, scope_id, active_corpus_revision_id, manifest_hash, generation
  ) VALUES (
    manifest_id, created_user_id, root_scope_id,
    initial_revision_id, p_empty_manifest_hash, 1
  );
  INSERT INTO straylight.active_manifest_history (
    id, user_id, scope_id, manifest_id, generation, corpus_revision_id,
    manifest_hash, change_kind
  ) VALUES (
    gen_random_uuid(), created_user_id, root_scope_id, manifest_id, 1,
    initial_revision_id, p_empty_manifest_hash, 'initial'
  );
  INSERT INTO straylight.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    straylight_auth.current_user_id(),
    straylight_auth.current_credential_id(),
    'admin.user.provision',
    jsonb_build_object('target_user_id', created_user_id),
    true
  );
  RETURN QUERY SELECT
    created_user_id, created_credential_id, root_scope_id,
    default_policy_id, initial_revision_id;
END;
$$;

CREATE TABLE straylight.task_contexts (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  slug text NOT NULL CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$' AND length(slug) <= 80),
  display_name text NOT NULL CHECK (length(btrim(display_name)) BETWEEN 1 AND 120),
  description text CHECK (description IS NULL OR length(description) <= 1000),
  archived_at timestamptz,
  created_by text NOT NULL DEFAULT 'owner' CHECK (
    created_by IN ('owner', 'todoist', 'derived') OR created_by ~ '^agent:[^[:space:]]+$'
  ),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, slug)
);

CREATE TABLE straylight.task_context_aliases (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  alias text NOT NULL CHECK (length(btrim(alias)) BETWEEN 1 AND 120),
  context_slug text NOT NULL,
  reason text NOT NULL DEFAULT 'owner' CHECK (length(reason) <= 120),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, alias),
  FOREIGN KEY (user_id, context_slug)
    REFERENCES straylight.task_contexts(user_id, slug) ON DELETE CASCADE
);

CREATE UNIQUE INDEX task_context_aliases_normalized_idx
  ON straylight.task_context_aliases (user_id, lower(alias));

CREATE TABLE straylight.task_projects (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  slug text NOT NULL CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$' AND length(slug) <= 100),
  title text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
  description text CHECK (description IS NULL OR length(description) <= 2000),
  hub_path text CHECK (hub_path IS NULL OR (hub_path <> '' AND hub_path !~ '^/')),
  repo_path text CHECK (repo_path IS NULL OR length(btrim(repo_path)) > 0),
  interest_override text CHECK (interest_override IN ('hot', 'normal', 'parked')),
  interest_set_by text CHECK (
    interest_set_by IS NULL OR interest_set_by IN ('owner', 'derived')
      OR interest_set_by ~ '^agent:[^[:space:]]+$'
  ),
  interest_set_at timestamptz,
  last_activity_at timestamptz,
  archived_at timestamptz,
  created_by text NOT NULL DEFAULT 'owner' CHECK (
    created_by IN ('owner', 'todoist', 'derived') OR created_by ~ '^agent:[^[:space:]]+$'
  ),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, slug),
  CHECK (
    (interest_override IS NULL) = (interest_set_by IS NULL)
    AND (interest_override IS NULL) = (interest_set_at IS NULL)
  )
);

CREATE TABLE straylight.task_project_aliases (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  alias text NOT NULL CHECK (length(btrim(alias)) BETWEEN 1 AND 160),
  project_slug text NOT NULL,
  reason text NOT NULL DEFAULT 'owner' CHECK (length(reason) <= 120),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, alias),
  FOREIGN KEY (user_id, project_slug)
    REFERENCES straylight.task_projects(user_id, slug) ON DELETE CASCADE
);

CREATE UNIQUE INDEX task_project_aliases_normalized_idx
  ON straylight.task_project_aliases (user_id, lower(alias));

CREATE TABLE straylight.task_index (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  task_id uuid NOT NULL,
  entry_id uuid NOT NULL,
  entry_version bigint NOT NULL CHECK (entry_version > 0),
  title text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 500),
  status text NOT NULL CHECK (status IN ('open', 'waiting', 'done', 'dropped')),
  ready_at timestamptz,
  soft_due date,
  hard_due timestamptz,
  hard_due_lead_days integer CHECK (hard_due_lead_days BETWEEN 0 AND 3650),
  cost_amount_cents bigint CHECK (cost_amount_cents >= 0),
  cost_period text CHECK (cost_period IN ('day', 'week', 'month')),
  cost_flag boolean NOT NULL DEFAULT false,
  cost_since date,
  required_contexts text[] NOT NULL DEFAULT '{}'::text[] CHECK (
    array_position(required_contexts, NULL) IS NULL
  ),
  project_slug text,
  estimate_minutes integer CHECK (estimate_minutes BETWEEN 1 AND 10080),
  waiting_on jsonb CHECK (waiting_on IS NULL OR jsonb_typeof(waiting_on) = 'object'),
  snooze_count integer NOT NULL DEFAULT 0 CHECK (snooze_count >= 0),
  parked boolean NOT NULL DEFAULT false,
  today_pin date,
  triaged_at timestamptz,
  done_at timestamptz,
  dropped_at timestamptz,
  recurrence jsonb,
  provenance jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(provenance) = 'object'),
  source_timestamps jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(source_timestamps) = 'object'),
  task jsonb NOT NULL CHECK (jsonb_typeof(task) = 'object'),
  created_at timestamptz NOT NULL,
  updated_at timestamptz NOT NULL,
  PRIMARY KEY (user_id, task_id),
  UNIQUE (user_id, entry_id),
  FOREIGN KEY (user_id, entry_id, entry_version)
    REFERENCES straylight.entry_versions(user_id, entry_id, version) ON DELETE CASCADE,
  FOREIGN KEY (user_id, project_slug)
    REFERENCES straylight.task_projects(user_id, slug)
);

CREATE INDEX task_index_candidates_idx
  ON straylight.task_index (user_id, parked, status, ready_at, created_at, task_id)
  WHERE status = 'open' AND NOT parked;
CREATE INDEX task_index_project_idx
  ON straylight.task_index (user_id, project_slug, status, updated_at DESC)
  WHERE project_slug IS NOT NULL;
CREATE INDEX task_index_done_idx
  ON straylight.task_index (user_id, done_at DESC, task_id)
  WHERE status = 'done';
CREATE INDEX task_index_contexts_idx
  ON straylight.task_index USING gin (required_contexts);

CREATE TABLE straylight.task_external_refs (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  system text NOT NULL CHECK (system ~ '^[a-z][a-z0-9._-]{0,63}$'),
  external_id text NOT NULL CHECK (length(external_id) BETWEEN 1 AND 512),
  task_id uuid NOT NULL,
  entry_id uuid NOT NULL,
  series_id text,
  occurrence_key text,
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(metadata) = 'object'),
  first_seen_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  last_seen_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, system, external_id),
  FOREIGN KEY (user_id, entry_id)
    REFERENCES straylight.entries(user_id, id),
  CHECK ((series_id IS NULL) = (occurrence_key IS NULL))
);

CREATE UNIQUE INDEX task_external_refs_occurrence_idx
  ON straylight.task_external_refs (user_id, system, series_id, occurrence_key)
  WHERE series_id IS NOT NULL;

CREATE TABLE straylight.task_corrections (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  task_id uuid NOT NULL,
  entry_id uuid NOT NULL,
  entry_version bigint NOT NULL CHECK (entry_version > 0),
  field_name text NOT NULL CHECK (field_name ~ '^[a-z][a-z0-9_]{0,79}$'),
  previous_value jsonb,
  previous_source text,
  corrected_value jsonb,
  corrected_source text NOT NULL,
  reason text CHECK (reason IS NULL OR length(reason) <= 1000),
  credential_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  FOREIGN KEY (user_id, entry_id, entry_version)
    REFERENCES straylight.entry_versions(user_id, entry_id, version),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id)
);

CREATE INDEX task_corrections_recent_idx
  ON straylight.task_corrections (user_id, created_at DESC, id);

CREATE TABLE straylight.task_audit_events (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  task_id uuid,
  credential_id uuid,
  action text NOT NULL CHECK (action ~ '^[a-z][a-z0-9_.-]{0,119}$'),
  details jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(details) = 'object'),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id)
);

CREATE INDEX task_audit_events_recent_idx
  ON straylight.task_audit_events (user_id, created_at DESC, id);

CREATE TABLE straylight.task_surface_defaults (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  surface text NOT NULL CHECK (surface ~ '^[a-z][a-z0-9._-]{0,63}$'),
  contexts text[] NOT NULL CHECK (
    cardinality(contexts) <= 20 AND array_position(contexts, NULL) IS NULL
  ),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, surface)
);

CREATE TABLE straylight.task_checkpoint_links (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  checkpoint_entry_id uuid NOT NULL,
  project_slug text NOT NULL,
  attribution text NOT NULL CHECK (attribution IN ('explicit', 'path_fallback')),
  matched_path text,
  linked_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, checkpoint_entry_id),
  FOREIGN KEY (user_id, checkpoint_entry_id)
    REFERENCES straylight.entries(user_id, id) ON DELETE CASCADE,
  FOREIGN KEY (user_id, project_slug)
    REFERENCES straylight.task_projects(user_id, slug)
);

CREATE INDEX task_checkpoint_links_project_idx
  ON straylight.task_checkpoint_links (user_id, project_slug, linked_at DESC);

CREATE TABLE straylight.task_settings (
  user_id uuid PRIMARY KEY REFERENCES straylight.users(id) ON DELETE CASCADE,
  timezone text NOT NULL DEFAULT 'UTC' CHECK (length(timezone) BETWEEN 1 AND 80),
  hard_lead_days integer NOT NULL DEFAULT 7 CHECK (hard_lead_days BETWEEN 1 AND 90),
  hard_second_lead_hours integer NOT NULL DEFAULT 48 CHECK (
    hard_second_lead_hours BETWEEN 1 AND 2160
  ),
  due_day_local_time time NOT NULL DEFAULT '07:00:00',
  soft_window_days integer NOT NULL DEFAULT 3 CHECK (soft_window_days BETWEEN 1 AND 90),
  triage_after_days integer NOT NULL DEFAULT 14 CHECK (triage_after_days BETWEEN 1 AND 3650),
  waiting_followup_days integer NOT NULL DEFAULT 7 CHECK (waiting_followup_days BETWEEN 1 AND 3650),
  quiet_override_enabled boolean NOT NULL DEFAULT true,
  quiet_override_within_hours integer NOT NULL DEFAULT 24 CHECK (
    quiet_override_within_hours BETWEEN 1 AND 168
  ),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE straylight.task_integration_config (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  system text NOT NULL CHECK (system ~ '^[a-z][a-z0-9._-]{0,63}$'),
  mode text NOT NULL DEFAULT 'off' CHECK (mode IN ('off', 'import_once', 'pull')),
  configuration_generation bigint NOT NULL DEFAULT 1 CHECK (configuration_generation > 0),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, system)
);

CREATE TABLE straylight.task_sync_state (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  system text NOT NULL CHECK (system ~ '^[a-z][a-z0-9._-]{0,63}$'),
  cursor text,
  configuration_generation bigint NOT NULL DEFAULT 1 CHECK (configuration_generation > 0),
  last_run_at timestamptz,
  last_outcome text,
  last_error_code text,
  next_run_at timestamptz,
  manual_requested_at timestamptz,
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, system)
);

CREATE OR REPLACE FUNCTION straylight.seed_task_user_defaults()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
BEGIN
  INSERT INTO straylight.task_contexts (user_id, slug, display_name, created_by)
  VALUES
    (NEW.id, 'phone', 'Phone', 'owner'),
    (NEW.id, 'home', 'Home', 'owner'),
    (NEW.id, 'errands', 'Errands', 'owner'),
    (NEW.id, 'quick', 'Quick', 'owner'),
    (NEW.id, 'online', 'Online', 'owner')
  ON CONFLICT DO NOTHING;
  INSERT INTO straylight.task_surface_defaults (user_id, surface, contexts)
  VALUES
    (NEW.id, 'ios', ARRAY['phone', 'online']::text[]),
    (NEW.id, 'web', ARRAY['online']::text[])
  ON CONFLICT DO NOTHING;
  INSERT INTO straylight.task_settings (user_id)
  VALUES (NEW.id)
  ON CONFLICT DO NOTHING;
  INSERT INTO straylight.task_integration_config (user_id, system, mode)
  VALUES (NEW.id, 'todoist', 'off')
  ON CONFLICT DO NOTHING;
  RETURN NEW;
END;
$$;

CREATE TRIGGER users_seed_task_defaults
AFTER INSERT ON straylight.users
FOR EACH ROW EXECUTE FUNCTION straylight.seed_task_user_defaults();

INSERT INTO straylight.task_contexts (user_id, slug, display_name, created_by)
SELECT users.id, seed.slug, seed.display_name, 'owner'
FROM straylight.users AS users
CROSS JOIN (VALUES
  ('phone', 'Phone'),
  ('home', 'Home'),
  ('errands', 'Errands'),
  ('quick', 'Quick'),
  ('online', 'Online')
) AS seed(slug, display_name)
ON CONFLICT DO NOTHING;

INSERT INTO straylight.task_surface_defaults (user_id, surface, contexts)
SELECT users.id, seed.surface, seed.contexts
FROM straylight.users AS users
CROSS JOIN (VALUES
  ('ios', ARRAY['phone', 'online']::text[]),
  ('web', ARRAY['online']::text[])
) AS seed(surface, contexts)
ON CONFLICT DO NOTHING;

INSERT INTO straylight.task_settings (user_id)
SELECT id FROM straylight.users
ON CONFLICT DO NOTHING;

INSERT INTO straylight.task_integration_config (user_id, system, mode)
SELECT id, 'todoist', 'off' FROM straylight.users
ON CONFLICT DO NOTHING;

CREATE TRIGGER task_corrections_immutable
BEFORE UPDATE OR DELETE ON straylight.task_corrections
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE TRIGGER task_audit_events_immutable
BEFORE UPDATE OR DELETE ON straylight.task_audit_events
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

DO $$
DECLARE
  table_name text;
BEGIN
  FOREACH table_name IN ARRAY ARRAY[
    'task_index',
    'task_contexts',
    'task_context_aliases',
    'task_projects',
    'task_project_aliases',
    'task_external_refs',
    'task_corrections',
    'task_audit_events',
    'task_surface_defaults',
    'task_checkpoint_links',
    'task_settings',
    'task_integration_config',
    'task_sync_state'
  ]
  LOOP
    EXECUTE format('ALTER TABLE straylight.%I ENABLE ROW LEVEL SECURITY', table_name);
    EXECUTE format('ALTER TABLE straylight.%I FORCE ROW LEVEL SECURITY', table_name);
    EXECUTE format(
      'CREATE POLICY task_user_select ON straylight.%I '
      'FOR SELECT TO app_rw, app_ro '
      'USING ('
      '  straylight_auth.can_access_user(user_id) '
      '  AND straylight_auth.has_any_capability('
      '    ARRAY[''task.read'', ''task.write'', ''integration.manage'', ''admin'']'
      '  )'
      ')',
      table_name
    );
  END LOOP;
END;
$$;

DO $$
DECLARE
  table_name text;
BEGIN
  FOREACH table_name IN ARRAY ARRAY[
    'task_index',
    'task_contexts',
    'task_context_aliases',
    'task_projects',
    'task_project_aliases',
    'task_external_refs',
    'task_corrections',
    'task_audit_events',
    'task_surface_defaults',
    'task_settings'
  ]
  LOOP
    EXECUTE format(
      'CREATE POLICY task_user_write ON straylight.%I '
      'FOR ALL TO app_rw '
      'USING ('
      '  straylight_auth.can_access_user(user_id) '
      '  AND straylight_auth.has_any_capability('
      '    ARRAY[''task.write'', ''admin'']'
      '  )'
      ') '
      'WITH CHECK ('
      '  straylight_auth.can_access_user(user_id) '
      '  AND straylight_auth.has_any_capability('
      '    ARRAY[''task.write'', ''admin'']'
      '  )'
      ')',
      table_name
    );
  END LOOP;
END;
$$;

CREATE POLICY task_checkpoint_links_write
ON straylight.task_checkpoint_links
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['checkpoint', 'task.write', 'admin']
  )
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['checkpoint', 'task.write', 'admin']
  )
);

CREATE POLICY task_sync_state_write
ON straylight.task_sync_state
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['integration.manage', 'admin'])
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['integration.manage', 'admin'])
);

CREATE POLICY task_integration_config_write
ON straylight.task_integration_config
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['integration.manage', 'admin'])
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['integration.manage', 'admin'])
);

CREATE POLICY checkpoint_projects_select
ON straylight.task_projects
FOR SELECT TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['checkpoint', 'admin'])
);

-- The original shared-core policies trusted handler capability checks and
-- selected by user only. Task-only credentials need entry access for their
-- canonical files without gaining read access to the rest of the workspace.
DROP POLICY simple_user_select ON straylight.entries;
DROP POLICY simple_user_select ON straylight.entry_versions;

CREATE POLICY workspace_entries_select ON straylight.entries
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'admin'
  ])
);

CREATE POLICY task_entries_select ON straylight.entries
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.read', 'task.write', 'admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-fA-F-]{36}\.md$'
);

CREATE POLICY workspace_entry_versions_select ON straylight.entry_versions
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'admin'
  ])
);

CREATE POLICY task_entry_versions_select ON straylight.entry_versions
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.read', 'task.write', 'admin'])
  AND EXISTS (
    SELECT 1 FROM straylight.entries AS entry
    WHERE entry.user_id=entry_versions.user_id
      AND entry.id=entry_versions.entry_id
      AND entry.path ~ '^\.straylight/tasks/[0-9a-fA-F-]{36}\.md$'
  )
);

-- task.write is intentionally restricted to canonical task paths in the
-- shared entry model. It never gains chunk or job mutation authority.
CREATE POLICY task_entries_insert ON straylight.entries
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write', 'admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-fA-F-]{36}\.md$'
);

CREATE POLICY task_entries_update ON straylight.entries
FOR UPDATE TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write', 'admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-fA-F-]{36}\.md$'
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write', 'admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-fA-F-]{36}\.md$'
);

CREATE POLICY task_entry_versions_insert ON straylight.entry_versions
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write', 'admin'])
  AND EXISTS (
    SELECT 1 FROM straylight.entries AS entry
    WHERE entry.user_id=entry_versions.user_id
      AND entry.id=entry_versions.entry_id
      AND entry.path ~ '^\.straylight/tasks/[0-9a-fA-F-]{36}\.md$'
  )
);

CREATE POLICY task_workspace_changes_insert ON straylight.workspace_changes
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write', 'admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-fA-F-]{36}\.md$'
  AND EXISTS (
    SELECT 1 FROM straylight.entries AS entry
    WHERE entry.user_id=workspace_changes.user_id
      AND entry.id=workspace_changes.entry_id
      AND entry.path=workspace_changes.path
  )
);

GRANT SELECT, INSERT, UPDATE, DELETE ON
  straylight.task_index,
  straylight.task_contexts,
  straylight.task_context_aliases,
  straylight.task_projects,
  straylight.task_project_aliases,
  straylight.task_external_refs,
  straylight.task_corrections,
  straylight.task_audit_events,
  straylight.task_surface_defaults,
  straylight.task_checkpoint_links,
  straylight.task_settings,
  straylight.task_integration_config,
  straylight.task_sync_state
TO app_rw;

GRANT SELECT ON
  straylight.task_index,
  straylight.task_contexts,
  straylight.task_context_aliases,
  straylight.task_projects,
  straylight.task_project_aliases,
  straylight.task_external_refs,
  straylight.task_corrections,
  straylight.task_audit_events,
  straylight.task_surface_defaults,
  straylight.task_checkpoint_links,
  straylight.task_settings,
  straylight.task_integration_config,
  straylight.task_sync_state
TO app_ro;

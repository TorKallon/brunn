-- Durable agent messaging. Canonical conversation state remains in versioned
-- workspace entries; the messaging tables are principals, bindings, compact
-- coordination state, and one rebuildable message projection.

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
      'message.read', 'message.write',
      'admin'
    ]::text[]
  );

ALTER TABLE straylight.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_owner_full_capabilities_check;

UPDATE straylight.api_credentials
SET capabilities = capabilities || ARRAY['message.read']::text[]
WHERE capabilities @> ARRAY['read']::text[]
  AND NOT capabilities @> ARRAY['message.read']::text[];

UPDATE straylight.api_credentials
SET capabilities = capabilities || ARRAY['message.write']::text[]
WHERE capabilities @> ARRAY['read', 'save']::text[]
  AND NOT capabilities @> ARRAY['message.write']::text[];

ALTER TABLE straylight.api_credentials
  ADD CONSTRAINT api_credentials_owner_full_capabilities_check CHECK (
    NOT capabilities @> ARRAY['credential:manage']::text[]
    OR capabilities @> ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'notification:publish', 'notification:manage',
      'secret:read', 'secret:write',
      'task.read', 'task.write', 'integration.manage',
      'message.read', 'message.write',
      'admin'
    ]::text[]
  );

-- These replacements are the 0071 task-aware definitions with only the two
-- messaging capabilities added. Keep task capability issuance intact.
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
    'message.read', 'message.write',
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
    'message.read', 'message.write',
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
    'message.read', 'message.write',
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

CREATE TABLE straylight.messaging_agents (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  agent_id text NOT NULL CHECK (
    length(agent_id) BETWEEN 1 AND 80
    AND agent_id ~ '^[a-z0-9]+([._-][a-z0-9]+)*$'
  ),
  display_name text NOT NULL CHECK (length(btrim(display_name)) BETWEEN 1 AND 120),
  principal_kind text NOT NULL CHECK (
    principal_kind IN ('resident', 'task-time', 'owner')
  ),
  delivery_mode text NOT NULL DEFAULT 'pull' CHECK (
    delivery_mode IN ('pull', 'apns', 'webhook')
  ),
  lease_expires_at timestamptz,
  last_seen_at timestamptz,
  created_by_credential_id uuid NOT NULL,
  archived_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, agent_id),
  FOREIGN KEY (user_id, created_by_credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  CHECK (lease_expires_at IS NULL OR last_seen_at IS NOT NULL),
  CHECK (last_seen_at IS NULL OR lease_expires_at IS NULL OR lease_expires_at >= last_seen_at)
);

CREATE TABLE straylight.messaging_credential_bindings (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  credential_id uuid NOT NULL,
  agent_id text NOT NULL,
  bound_by_credential_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, credential_id),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id) ON DELETE CASCADE,
  FOREIGN KEY (user_id, agent_id)
    REFERENCES straylight.messaging_agents(user_id, agent_id) ON DELETE CASCADE,
  FOREIGN KEY (user_id, bound_by_credential_id)
    REFERENCES straylight.api_credentials(user_id, id)
);

CREATE INDEX messaging_credential_bindings_agent_idx
  ON straylight.messaging_credential_bindings (user_id, agent_id, credential_id);

CREATE TABLE straylight.messaging_sync_state (
  user_id uuid PRIMARY KEY REFERENCES straylight.users(id) ON DELETE CASCADE,
  current_cursor bigint NOT NULL DEFAULT 0 CHECK (current_cursor >= 0),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE straylight.messaging_conversations (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  conversation_id uuid NOT NULL,
  entry_id uuid NOT NULL,
  path text NOT NULL,
  conversation_kind text NOT NULL CHECK (conversation_kind IN ('direct', 'group')),
  direct_key text,
  subject text CHECK (subject IS NULL OR length(btrim(subject)) BETWEEN 1 AND 240),
  status text NOT NULL DEFAULT 'open' CHECK (
    status IN ('open', 'paused_for_human', 'closed')
  ),
  created_by_agent_id text NOT NULL,
  last_seq bigint NOT NULL DEFAULT 0 CHECK (last_seq >= 0),
  last_message_at timestamptz,
  agent_streak integer NOT NULL DEFAULT 0 CHECK (agent_streak BETWEEN 0 AND 20),
  needs_human boolean NOT NULL DEFAULT false,
  continues_from uuid,
  latest_sync_cursor bigint NOT NULL DEFAULT 0 CHECK (latest_sync_cursor >= 0),
  closed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, conversation_id),
  UNIQUE (user_id, entry_id),
  UNIQUE (user_id, path),
  FOREIGN KEY (user_id, entry_id)
    REFERENCES straylight.entries(user_id, id) ON DELETE CASCADE
    DEFERRABLE INITIALLY DEFERRED,
  FOREIGN KEY (user_id, created_by_agent_id)
    REFERENCES straylight.messaging_agents(user_id, agent_id),
  FOREIGN KEY (user_id, continues_from)
    REFERENCES straylight.messaging_conversations(user_id, conversation_id),
  CHECK (
    path = '.straylight/conversations/' || conversation_id::text || '.md'
  ),
  CHECK (
    (conversation_kind = 'direct' AND direct_key IS NOT NULL
      AND length(btrim(direct_key)) BETWEEN 1 AND 200)
    OR (conversation_kind = 'group' AND direct_key IS NULL)
  ),
  CHECK ((last_seq = 0) = (last_message_at IS NULL)),
  CHECK (status <> 'paused_for_human' OR needs_human),
  CHECK ((status = 'closed') = (closed_at IS NOT NULL)),
  CHECK (continues_from IS NULL OR continues_from <> conversation_id)
);

CREATE UNIQUE INDEX messaging_conversations_active_direct_idx
  ON straylight.messaging_conversations (user_id, direct_key)
  WHERE conversation_kind = 'direct' AND status IN ('open', 'paused_for_human');

CREATE UNIQUE INDEX messaging_conversations_continuation_idx
  ON straylight.messaging_conversations (user_id, continues_from)
  WHERE continues_from IS NOT NULL;

CREATE INDEX messaging_conversations_sync_idx
  ON straylight.messaging_conversations (
    user_id, latest_sync_cursor, conversation_id
  );

CREATE INDEX messaging_conversations_inbox_idx
  ON straylight.messaging_conversations (
    user_id, status, last_message_at DESC, conversation_id
  );

CREATE TABLE straylight.messaging_participants (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  conversation_id uuid NOT NULL,
  agent_id text NOT NULL,
  role text NOT NULL CHECK (role IN ('participant', 'observer')),
  last_read_seq bigint NOT NULL DEFAULT 0 CHECK (last_read_seq >= 0),
  joined_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, conversation_id, agent_id),
  FOREIGN KEY (user_id, conversation_id)
    REFERENCES straylight.messaging_conversations(user_id, conversation_id)
    ON DELETE CASCADE,
  FOREIGN KEY (user_id, agent_id)
    REFERENCES straylight.messaging_agents(user_id, agent_id)
    ON DELETE CASCADE
);

CREATE INDEX messaging_participants_agent_inbox_idx
  ON straylight.messaging_participants (
    user_id, agent_id, conversation_id, last_read_seq
  );

CREATE TABLE straylight.messaging_message_index (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  conversation_id uuid NOT NULL,
  seq bigint NOT NULL CHECK (seq > 0),
  message_id uuid NOT NULL,
  from_agent_id text,
  client_key text,
  system_key text,
  request_hash straylight.sha256_hex,
  kind text NOT NULL CHECK (kind IN ('text', 'question', 'system')),
  body_md text NOT NULL CHECK (octet_length(body_md) BETWEEN 1 AND 16384),
  refs jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (
    jsonb_typeof(refs) = 'array' AND jsonb_array_length(refs) <= 32
  ),
  in_reply_to bigint CHECK (in_reply_to IS NULL OR in_reply_to > 0),
  correlation_id text CHECK (
    correlation_id IS NULL OR length(btrim(correlation_id)) BETWEEN 1 AND 200
  ),
  expects_reply boolean NOT NULL DEFAULT false,
  reply_by timestamptz,
  reply_by_handled_at timestamptz,
  sync_cursor bigint NOT NULL CHECK (sync_cursor > 0),
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, conversation_id, seq),
  UNIQUE (user_id, conversation_id, message_id),
  UNIQUE (user_id, message_id),
  UNIQUE (user_id, from_agent_id, client_key),
  UNIQUE (user_id, sync_cursor),
  FOREIGN KEY (user_id, conversation_id)
    REFERENCES straylight.messaging_conversations(user_id, conversation_id)
    ON DELETE CASCADE,
  FOREIGN KEY (user_id, from_agent_id)
    REFERENCES straylight.messaging_agents(user_id, agent_id),
  FOREIGN KEY (user_id, conversation_id, in_reply_to)
    REFERENCES straylight.messaging_message_index(user_id, conversation_id, seq)
    DEFERRABLE INITIALLY DEFERRED,
  CHECK (
    (kind = 'system' AND from_agent_id IS NULL
      AND client_key IS NULL AND request_hash IS NULL
      AND system_key IS NOT NULL
      AND system_key = btrim(system_key)
      AND length(system_key) BETWEEN 1 AND 200)
    OR
    (kind <> 'system' AND from_agent_id IS NOT NULL
      AND client_key ~ '^[0-9A-HJKMNP-TV-Z]{26}$'
      AND request_hash IS NOT NULL AND system_key IS NULL)
  ),
  CHECK (NOT expects_reply OR kind = 'question'),
  CHECK (reply_by IS NULL OR expects_reply),
  CHECK (
    reply_by IS NULL
    OR (reply_by > created_at AND reply_by <= created_at + interval '24 hours')
  ),
  CHECK (reply_by_handled_at IS NULL OR reply_by IS NOT NULL)
);

CREATE INDEX messaging_message_index_cursor_idx
  ON straylight.messaging_message_index (
    user_id, sync_cursor, conversation_id, seq
  );

CREATE UNIQUE INDEX messaging_message_index_system_key_idx
  ON straylight.messaging_message_index (
    user_id, conversation_id, system_key
  )
  WHERE system_key IS NOT NULL;

CREATE INDEX messaging_message_index_sender_rate_idx
  ON straylight.messaging_message_index (user_id, from_agent_id, created_at DESC)
  WHERE from_agent_id IS NOT NULL;

CREATE INDEX messaging_message_index_conversation_rate_idx
  ON straylight.messaging_message_index (user_id, conversation_id, created_at DESC)
  WHERE kind <> 'system';

CREATE INDEX messaging_message_index_reply_by_idx
  ON straylight.messaging_message_index (
    reply_by, user_id, conversation_id, seq
  )
  WHERE reply_by IS NOT NULL AND reply_by_handled_at IS NULL;

DO $$
DECLARE
  table_name text;
BEGIN
  FOREACH table_name IN ARRAY ARRAY[
    'messaging_agents',
    'messaging_credential_bindings',
    'messaging_sync_state',
    'messaging_conversations',
    'messaging_participants',
    'messaging_message_index'
  ]
  LOOP
    EXECUTE format('ALTER TABLE straylight.%I ENABLE ROW LEVEL SECURITY', table_name);
    EXECUTE format('ALTER TABLE straylight.%I FORCE ROW LEVEL SECURITY', table_name);
  END LOOP;
END;
$$;

CREATE OR REPLACE FUNCTION straylight.enforce_messaging_agent_update()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, straylight, straylight_auth
AS $$
BEGIN
  IF ROW(
       OLD.user_id,
       OLD.agent_id,
       OLD.display_name,
       OLD.principal_kind,
       OLD.delivery_mode,
       OLD.created_by_credential_id,
       OLD.archived_at,
       OLD.created_at
     ) IS DISTINCT FROM ROW(
       NEW.user_id,
       NEW.agent_id,
       NEW.display_name,
       NEW.principal_kind,
       NEW.delivery_mode,
       NEW.created_by_credential_id,
       NEW.archived_at,
       NEW.created_at
     )
     AND NOT straylight_auth.has_any_capability(
       ARRAY['credential:manage', 'admin']
     ) THEN
    RAISE EXCEPTION 'message.read can update only messaging presence'
      USING ERRCODE = '42501';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER messaging_agents_update_guard
BEFORE UPDATE ON straylight.messaging_agents
FOR EACH ROW EXECUTE FUNCTION straylight.enforce_messaging_agent_update();

CREATE OR REPLACE FUNCTION straylight.enforce_messaging_participant_update()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, straylight, straylight_auth
AS $$
BEGIN
  IF NEW.last_read_seq < OLD.last_read_seq THEN
    RAISE EXCEPTION 'messaging read position cannot move backward'
      USING ERRCODE = '22023';
  END IF;
  IF ROW(
       OLD.user_id,
       OLD.conversation_id,
       OLD.agent_id,
       OLD.role,
       OLD.joined_at
     ) IS DISTINCT FROM ROW(
       NEW.user_id,
       NEW.conversation_id,
       NEW.agent_id,
       NEW.role,
       NEW.joined_at
     )
     AND NOT straylight_auth.has_any_capability(
       ARRAY['message.write', 'admin']
     ) THEN
    RAISE EXCEPTION 'message.read can update only its durable read position'
      USING ERRCODE = '42501';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER messaging_participants_update_guard
BEFORE UPDATE ON straylight.messaging_participants
FOR EACH ROW EXECUTE FUNCTION straylight.enforce_messaging_participant_update();

CREATE POLICY messaging_agents_select
ON straylight.messaging_agents
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY[
    'message.read', 'message.write', 'credential:manage', 'admin'
  ])
);

CREATE POLICY messaging_agents_presence_update
ON straylight.messaging_agents
FOR UPDATE TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['message.read', 'message.write', 'admin']
  )
  AND EXISTS (
    SELECT 1
    FROM straylight.messaging_credential_bindings AS binding
    WHERE binding.user_id=messaging_agents.user_id
      AND binding.credential_id=straylight_auth.current_credential_id()
      AND binding.agent_id=messaging_agents.agent_id
  )
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['message.read', 'message.write', 'admin']
  )
  AND EXISTS (
    SELECT 1
    FROM straylight.messaging_credential_bindings AS binding
    WHERE binding.user_id=messaging_agents.user_id
      AND binding.credential_id=straylight_auth.current_credential_id()
      AND binding.agent_id=messaging_agents.agent_id
  )
);

CREATE POLICY messaging_agents_registry_insert
ON straylight.messaging_agents
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['credential:manage', 'admin'])
);

CREATE POLICY messaging_agents_registry_update
ON straylight.messaging_agents
FOR UPDATE TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['credential:manage', 'admin'])
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['credential:manage', 'admin'])
);

CREATE POLICY messaging_bindings_select
ON straylight.messaging_credential_bindings
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND (
    straylight_auth.has_any_capability(ARRAY['credential:manage', 'admin'])
    OR (
      credential_id=straylight_auth.current_credential_id()
      AND straylight_auth.has_any_capability(
        ARRAY['message.read', 'message.write']
      )
    )
  )
);

CREATE POLICY messaging_bindings_write
ON straylight.messaging_credential_bindings
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['credential:manage', 'admin'])
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['credential:manage', 'admin'])
);

CREATE POLICY messaging_conversations_select
ON straylight.messaging_conversations
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['message.read', 'message.write', 'admin']
  )
);

CREATE POLICY messaging_conversations_write
ON straylight.messaging_conversations
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
);

CREATE POLICY messaging_participants_select
ON straylight.messaging_participants
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['message.read', 'message.write', 'admin']
  )
);

CREATE POLICY messaging_participants_read_state_update
ON straylight.messaging_participants
FOR UPDATE TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_capability('message.read')
  AND EXISTS (
    SELECT 1
    FROM straylight.messaging_credential_bindings AS binding
    WHERE binding.user_id=messaging_participants.user_id
      AND binding.credential_id=straylight_auth.current_credential_id()
      AND binding.agent_id=messaging_participants.agent_id
  )
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_capability('message.read')
  AND EXISTS (
    SELECT 1
    FROM straylight.messaging_credential_bindings AS binding
    WHERE binding.user_id=messaging_participants.user_id
      AND binding.credential_id=straylight_auth.current_credential_id()
      AND binding.agent_id=messaging_participants.agent_id
  )
);

CREATE POLICY messaging_participants_write
ON straylight.messaging_participants
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
);

CREATE POLICY messaging_message_index_select
ON straylight.messaging_message_index
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['message.read', 'message.write', 'admin']
  )
);

CREATE POLICY messaging_message_index_write
ON straylight.messaging_message_index
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
);

CREATE POLICY messaging_sync_state_select
ON straylight.messaging_sync_state
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['message.read', 'message.write', 'admin']
  )
);

CREATE POLICY messaging_sync_state_write
ON straylight.messaging_sync_state
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
);

-- Messaging-only credentials can see and version only typed canonical
-- conversation entries. They never gain search-chunk or job authority.
CREATE POLICY messaging_entries_select ON straylight.entries
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['message.read', 'message.write', 'admin']
  )
  AND path ~ '^\.straylight/conversations/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.md$'
);

CREATE POLICY messaging_entry_versions_select ON straylight.entry_versions
FOR SELECT TO app_rw, app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['message.read', 'message.write', 'admin']
  )
  AND jsonb_typeof(
    CASE
      WHEN jsonb_typeof(metadata->'client') = 'object' THEN metadata->'client'
      ELSE metadata
    END
  ) = 'object'
  AND (
    CASE
      WHEN jsonb_typeof(metadata->'client') = 'object' THEN metadata->'client'
      ELSE metadata
    END
  )->>'kind' = 'conversation'
  AND (
    CASE
      WHEN jsonb_typeof(metadata->'client') = 'object' THEN metadata->'client'
      ELSE metadata
    END
  )->>'schema' = 'conversation.v1'
  AND jsonb_typeof(
    (
      CASE
        WHEN jsonb_typeof(metadata->'client') = 'object' THEN metadata->'client'
        ELSE metadata
      END
    )->'conversation'
  ) = 'object'
  AND EXISTS (
    SELECT 1 FROM straylight.entries AS entry
    WHERE entry.user_id=entry_versions.user_id
      AND entry.id=entry_versions.entry_id
      AND entry.path ~ '^\.straylight/conversations/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.md$'
      AND (
        CASE
          WHEN jsonb_typeof(entry_versions.metadata->'client') = 'object'
            THEN entry_versions.metadata->'client'
          ELSE entry_versions.metadata
        END
      )->'conversation'->>'id' = substring(
        entry.path from '^\.straylight/conversations/([0-9a-f-]{36})\.md$'
      )
  )
);

CREATE POLICY messaging_entries_insert ON straylight.entries
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
  AND path ~ '^\.straylight/conversations/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.md$'
);

CREATE POLICY messaging_entries_update ON straylight.entries
FOR UPDATE TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
  AND path ~ '^\.straylight/conversations/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.md$'
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
  AND path ~ '^\.straylight/conversations/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.md$'
);

CREATE POLICY messaging_entry_versions_insert ON straylight.entry_versions
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
  AND jsonb_typeof(
    CASE
      WHEN jsonb_typeof(metadata->'client') = 'object' THEN metadata->'client'
      ELSE metadata
    END
  ) = 'object'
  AND (
    CASE
      WHEN jsonb_typeof(metadata->'client') = 'object' THEN metadata->'client'
      ELSE metadata
    END
  )->>'kind' = 'conversation'
  AND (
    CASE
      WHEN jsonb_typeof(metadata->'client') = 'object' THEN metadata->'client'
      ELSE metadata
    END
  )->>'schema' = 'conversation.v1'
  AND jsonb_typeof(
    (
      CASE
        WHEN jsonb_typeof(metadata->'client') = 'object' THEN metadata->'client'
        ELSE metadata
      END
    )->'conversation'
  ) = 'object'
  AND EXISTS (
    SELECT 1 FROM straylight.entries AS entry
    WHERE entry.user_id=entry_versions.user_id
      AND entry.id=entry_versions.entry_id
      AND entry.path ~ '^\.straylight/conversations/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.md$'
      AND (
        CASE
          WHEN jsonb_typeof(entry_versions.metadata->'client') = 'object'
            THEN entry_versions.metadata->'client'
          ELSE entry_versions.metadata
        END
      )->'conversation'->>'id' = substring(
        entry.path from '^\.straylight/conversations/([0-9a-f-]{36})\.md$'
      )
  )
);

CREATE POLICY messaging_workspace_changes_insert ON straylight.workspace_changes
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['message.write', 'admin'])
  AND path ~ '^\.straylight/conversations/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.md$'
  AND EXISTS (
    SELECT 1 FROM straylight.entries AS entry
    WHERE entry.user_id=workspace_changes.user_id
      AND entry.id=workspace_changes.entry_id
      AND entry.path=workspace_changes.path
  )
);

GRANT SELECT, INSERT, UPDATE, DELETE ON
  straylight.messaging_agents,
  straylight.messaging_credential_bindings,
  straylight.messaging_sync_state,
  straylight.messaging_conversations,
  straylight.messaging_participants,
  straylight.messaging_message_index
TO app_rw;

GRANT SELECT ON
  straylight.messaging_agents,
  straylight.messaging_credential_bindings,
  straylight.messaging_sync_state,
  straylight.messaging_conversations,
  straylight.messaging_participants,
  straylight.messaging_message_index
TO app_ro;

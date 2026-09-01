-- One-way Todoist Unified API v1 pull. The worker reads exactly one named
-- secret through a non-public primitive, owns a narrow non-bearer producer,
-- and leases durable sync-state rows without holding a transaction across the
-- upstream request.

ALTER TABLE brunn.task_sync_state
  ADD COLUMN lease_owner text,
  ADD COLUMN lease_expires_at timestamptz,
  ADD CONSTRAINT task_sync_state_lease_pair_check CHECK (
    (lease_owner IS NULL) = (lease_expires_at IS NULL)
    AND (lease_owner IS NULL OR length(lease_owner) BETWEEN 1 AND 200)
  ),
  ADD CONSTRAINT task_sync_state_cursor_bound_check CHECK (
    cursor IS NULL OR length(cursor) BETWEEN 1 AND 16384
  ),
  ADD CONSTRAINT task_sync_state_error_code_check CHECK (
    last_error_code IS NULL
    OR last_error_code ~ '^[a-z][a-z0-9._-]{0,119}$'
  );

CREATE INDEX task_todoist_sync_due_idx
  ON brunn.task_sync_state (
    COALESCE(manual_requested_at,next_run_at),user_id
  )
  WHERE system='todoist';

-- Incremental Sync responses include only changed projects, while later item
-- deltas still identify their project by ID. Keep the last complete ID/name
-- mapping as private integration state so project routing stays deterministic.
CREATE TABLE brunn.task_todoist_projects (
  user_id uuid NOT NULL REFERENCES brunn.users(id) ON DELETE CASCADE,
  external_id text NOT NULL CHECK (length(external_id) BETWEEN 1 AND 512),
  name text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 200),
  is_deleted boolean NOT NULL DEFAULT false,
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id,external_id)
);

ALTER TABLE brunn.task_todoist_projects ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.task_todoist_projects FORCE ROW LEVEL SECURITY;
REVOKE ALL ON brunn.task_todoist_projects FROM PUBLIC,app_rw,app_ro;

-- Todoist reuses one item ID while advancing a recurring due value. Preserve
-- every canonical occurrence separately while task_external_refs continues to
-- point the stable external ID at the current occurrence.
CREATE TABLE brunn.task_todoist_occurrences (
  user_id uuid NOT NULL REFERENCES brunn.users(id) ON DELETE CASCADE,
  series_id text NOT NULL CHECK (length(series_id) BETWEEN 1 AND 512),
  occurrence_key text NOT NULL CHECK (length(occurrence_key) BETWEEN 1 AND 512),
  task_id uuid NOT NULL,
  entry_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id,series_id,occurrence_key),
  UNIQUE (user_id,task_id),
  FOREIGN KEY (user_id,entry_id)
    REFERENCES brunn.entries(user_id,id) ON DELETE CASCADE
);

ALTER TABLE brunn.task_todoist_occurrences ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.task_todoist_occurrences FORCE ROW LEVEL SECURITY;

CREATE POLICY task_todoist_occurrences_select
ON brunn.task_todoist_occurrences
FOR SELECT TO app_rw,app_ro
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(
    ARRAY['task.read','task.write','integration.manage','admin']
  )
);

CREATE POLICY task_todoist_occurrences_write
ON brunn.task_todoist_occurrences
FOR ALL TO app_rw
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['task.write','admin'])
)
WITH CHECK (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['task.write','admin'])
);

GRANT SELECT,INSERT,UPDATE,DELETE ON brunn.task_todoist_occurrences TO app_rw;
GRANT SELECT ON brunn.task_todoist_occurrences TO app_ro;

CREATE OR REPLACE FUNCTION brunn.seed_todoist_sync_state()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
BEGIN
  INSERT INTO brunn.task_sync_state (
    user_id,system,configuration_generation
  ) VALUES (NEW.id,'todoist',1)
  ON CONFLICT (user_id,system) DO NOTHING;
  INSERT INTO brunn.task_projects (
    user_id,slug,title,description,created_by
  ) VALUES (
    NEW.id,'todoist-inbox','Todoist Inbox',
    'Fallback for Todoist projects that are not mapped to the registry.',
    'todoist'
  ) ON CONFLICT (user_id,slug) DO NOTHING;
  RETURN NEW;
END;
$$;

CREATE TRIGGER users_seed_todoist_sync_state
AFTER INSERT ON brunn.users
FOR EACH ROW EXECUTE FUNCTION brunn.seed_todoist_sync_state();

INSERT INTO brunn.task_sync_state (
  user_id,system,configuration_generation
)
SELECT user_id,'todoist',configuration_generation
FROM brunn.task_integration_config
WHERE system='todoist'
ON CONFLICT (user_id,system) DO NOTHING;

INSERT INTO brunn.task_projects (
  user_id,slug,title,description,created_by
)
SELECT id,'todoist-inbox','Todoist Inbox',
       'Fallback for Todoist projects that are not mapped to the registry.',
       'todoist'
FROM brunn.users
ON CONFLICT (user_id,slug) DO NOTHING;

REVOKE ALL ON FUNCTION brunn.seed_todoist_sync_state()
FROM PUBLIC,app_rw,app_ro;

CREATE TABLE brunn.task_todoist_producers (
  user_id uuid PRIMARY KEY REFERENCES brunn.users(id) ON DELETE CASCADE,
  credential_id uuid NOT NULL UNIQUE,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  FOREIGN KEY (user_id,credential_id)
    REFERENCES brunn.api_credentials(user_id,id) ON DELETE CASCADE
);

ALTER TABLE brunn.task_todoist_producers ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.task_todoist_producers FORCE ROW LEVEL SECURITY;
REVOKE ALL ON brunn.task_todoist_producers FROM PUBLIC,app_rw,app_ro;

CREATE OR REPLACE FUNCTION brunn.ensure_task_todoist_producer(p_user_id uuid)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog,brunn
SET row_security = off
AS $$
DECLARE
  producer_id uuid;
BEGIN
  PERFORM pg_advisory_xact_lock(hashtextextended(
    'brunn.todoist.producer.v1|' || p_user_id::text,
    0
  ));
  SELECT credential_id INTO producer_id
  FROM brunn.task_todoist_producers
  WHERE user_id=p_user_id;
  IF producer_id IS NOT NULL THEN
    RETURN producer_id;
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM brunn.users
    WHERE id=p_user_id AND account_status='active'
  ) THEN
    RAISE EXCEPTION 'active Todoist sync user not found' USING ERRCODE='P0002';
  END IF;

  producer_id := gen_random_uuid();
  INSERT INTO brunn.api_credentials (
    id,user_id,label,token_hash,capabilities
  ) VALUES (
    producer_id,
    p_user_id,
    '__brunn_todoist_sync__',
    -- An already-hashed random non-bearer. No plaintext token exists or is
    -- returned, and it has no integration-management or secret capability.
    encode(public.gen_random_bytes(32),'hex'),
    ARRAY['task.read','task.write']::text[]
  );
  INSERT INTO brunn.task_todoist_producers (user_id,credential_id)
  VALUES (p_user_id,producer_id);
  RETURN producer_id;
END;
$$;

REVOKE ALL ON FUNCTION brunn.ensure_task_todoist_producer(uuid)
FROM PUBLIC,app_rw,app_ro;

-- This is the only worker secret read. It returns encrypted material for the
-- exact Todoist token name and records the ordinary content-free vault access
-- event before returning. Only the administrative worker connection may call
-- it; neither application database role receives EXECUTE.
CREATE OR REPLACE FUNCTION brunn.task_todoist_secret_for_worker(p_user_id uuid)
RETURNS TABLE (
  secret_id uuid,
  value_ciphertext bytea,
  value_nonce bytea,
  version integer,
  producer_credential_id uuid
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog,brunn
SET row_security = off
AS $$
DECLARE
  producer_id uuid;
  matched_secret brunn.secrets%ROWTYPE;
BEGIN
  SELECT * INTO matched_secret
  FROM brunn.secrets
  WHERE user_id=p_user_id AND name='todoist-api-token';
  IF NOT FOUND THEN
    RETURN;
  END IF;

  producer_id := brunn.ensure_task_todoist_producer(p_user_id);
  INSERT INTO brunn.secret_access_log (
    user_id,secret_id,credential_id,operation
  ) VALUES (p_user_id,matched_secret.id,producer_id,'get');

  RETURN QUERY SELECT
    matched_secret.id,
    matched_secret.value_ciphertext,
    matched_secret.value_nonce,
    matched_secret.version,
    producer_id;
END;
$$;

REVOKE ALL ON FUNCTION brunn.task_todoist_secret_for_worker(uuid)
FROM PUBLIC,app_rw,app_ro;

-- Saved mode and manual pull are owner-Web controls. A caller cannot emulate
-- this check with a bearer credential; Web credentials are non-bearer rows
-- owned by web_identities and unsafe session requests have already passed the
-- session-bound CSRF middleware.
CREATE OR REPLACE FUNCTION brunn.require_todoist_web_owner(p_user_id uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog,brunn,brunn_auth
SET row_security = off
AS $$
BEGIN
  IF NOT brunn_auth.can_access_user(p_user_id)
     OR NOT brunn_auth.has_any_capability(
       ARRAY['integration.manage','admin']
     )
     OR NOT EXISTS (
       SELECT 1 FROM brunn.web_identities AS identity
       WHERE identity.user_id=p_user_id
         AND identity.web_credential_id=brunn_auth.current_credential_id()
     ) THEN
    RAISE EXCEPTION 'Todoist integration management requires an owner Web session'
      USING ERRCODE='42501';
  END IF;
END;
$$;

REVOKE ALL ON FUNCTION brunn.require_todoist_web_owner(uuid)
FROM PUBLIC,app_ro;
GRANT EXECUTE ON FUNCTION brunn.require_todoist_web_owner(uuid) TO app_rw;

-- Internal Todoist credentials are absent from the public credential inventory
-- and cannot be revoked through the ordinary owner control primitive.
CREATE OR REPLACE FUNCTION brunn_auth.list_credentials(p_user_id uuid)
RETURNS TABLE (
  id uuid,
  label text,
  capabilities text[],
  scope_refs text[],
  created_at timestamptz,
  disabled_at timestamptz
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog,brunn,brunn_auth
SET row_security = off
AS $$
BEGIN
  IF NOT brunn_auth.context_is_valid()
     OR brunn_auth.current_user_id() IS DISTINCT FROM p_user_id
     OR NOT brunn_auth.has_any_capability(ARRAY['status','read']) THEN
    RAISE EXCEPTION 'authenticated same-user status or read capability is required'
      USING ERRCODE='42501';
  END IF;

  RETURN QUERY
  SELECT credential.id,
         credential.label::text,
         credential.capabilities,
         coalesce(
           array_agg(scope_row.scope_ref::text ORDER BY scope_row.scope_ref)
             FILTER (WHERE scope_row.id IS NOT NULL),
           '{}'::text[]
         ),
         credential.created_at,
         credential.disabled_at
  FROM brunn.api_credentials AS credential
  LEFT JOIN brunn.credential_scope_grants AS scope_grant
    ON scope_grant.user_id=credential.user_id
   AND scope_grant.credential_id=credential.id
  LEFT JOIN brunn.scopes AS scope_row
    ON scope_row.user_id=scope_grant.user_id
   AND scope_row.id=scope_grant.scope_id
  WHERE credential.user_id=p_user_id
    AND NOT EXISTS (
      SELECT 1 FROM brunn.web_identities AS identity
      WHERE identity.user_id=credential.user_id
        AND identity.web_credential_id=credential.id
    )
    AND NOT EXISTS (
      SELECT 1 FROM brunn.task_guard_producers AS guard
      WHERE guard.user_id=credential.user_id
        AND guard.credential_id=credential.id
    )
    AND NOT EXISTS (
      SELECT 1 FROM brunn.task_todoist_producers AS todoist
      WHERE todoist.user_id=credential.user_id
        AND todoist.credential_id=credential.id
    )
  GROUP BY credential.id,credential.label,credential.capabilities,
           credential.created_at,credential.disabled_at
  ORDER BY credential.created_at,credential.id;
END;
$$;

CREATE OR REPLACE FUNCTION brunn_auth.revoke_credential(
  p_user_id uuid,
  p_credential_id uuid
)
RETURNS timestamptz
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog,brunn,brunn_auth
SET row_security = off
AS $$
DECLARE
  revoked_at timestamptz;
BEGIN
  PERFORM brunn_auth.require_credential_control(p_user_id);

  UPDATE brunn.api_credentials AS credential
  SET disabled_at=coalesce(credential.disabled_at,clock_timestamp())
  WHERE credential.user_id=p_user_id
    AND credential.id=p_credential_id
    AND NOT EXISTS (
      SELECT 1 FROM brunn.web_identities AS identity
      WHERE identity.user_id=credential.user_id
        AND identity.web_credential_id=credential.id
    )
    AND NOT EXISTS (
      SELECT 1 FROM brunn.task_guard_producers AS guard
      WHERE guard.user_id=credential.user_id
        AND guard.credential_id=credential.id
    )
    AND NOT EXISTS (
      SELECT 1 FROM brunn.task_todoist_producers AS todoist
      WHERE todoist.user_id=credential.user_id
        AND todoist.credential_id=credential.id
    )
  RETURNING credential.disabled_at INTO revoked_at;

  IF revoked_at IS NULL THEN
    RAISE EXCEPTION 'credential not found for user' USING ERRCODE='P0002';
  END IF;

  INSERT INTO brunn.audit_events (
    user_id,scope_id,credential_id,action,details,content_free
  ) VALUES (
    p_user_id,NULL,brunn_auth.current_credential_id(),
    'auth.credential.revoke',
    jsonb_build_object('credential_id',p_credential_id,'revoked_at',revoked_at),
    true
  );
  RETURN revoked_at;
END;
$$;

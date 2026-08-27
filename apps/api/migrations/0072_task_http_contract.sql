-- Public task HTTP mutations use optimistic registry versions and one durable,
-- replayable operation receipt per user/kind/key. Handlers insert the pending
-- receipt before touching any compare-and-swap row, then finalize it in the
-- same transaction as the canonical entry/version/change.

ALTER TABLE straylight.task_contexts
  ADD COLUMN version bigint NOT NULL DEFAULT 1 CHECK (version > 0);

ALTER TABLE straylight.task_projects
  ADD COLUMN version bigint NOT NULL DEFAULT 1 CHECK (version > 0);

ALTER TABLE straylight.task_surface_defaults
  ADD COLUMN version bigint NOT NULL DEFAULT 1 CHECK (version > 0);

ALTER TABLE straylight.task_settings
  ADD COLUMN version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
  ADD COLUMN quiet_hours_start time NOT NULL DEFAULT '22:00:00',
  ADD COLUMN quiet_hours_end time NOT NULL DEFAULT '07:00:00';

CREATE TABLE straylight.task_operation_receipts (
  user_id uuid NOT NULL REFERENCES straylight.users(id) ON DELETE CASCADE,
  operation_kind text NOT NULL CHECK (
    operation_kind ~ '^[a-z][a-z0-9._-]{0,79}$'
  ),
  idempotency_key text NOT NULL CHECK (
    char_length(idempotency_key) BETWEEN 1 AND 240
    AND idempotency_key !~ '[[:cntrl:]]'
  ),
  request_hash straylight.sha256_hex NOT NULL,
  status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'committed')),
  task_id uuid,
  receipt jsonb,
  created_by_credential_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  committed_at timestamptz,
  PRIMARY KEY (user_id, operation_kind, idempotency_key),
  FOREIGN KEY (user_id, created_by_credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  CHECK (
    (status = 'pending' AND receipt IS NULL AND committed_at IS NULL)
    OR (
      status = 'committed'
      AND jsonb_typeof(receipt) = 'object'
      AND committed_at IS NOT NULL
    )
  )
);

CREATE INDEX task_operation_receipts_task_idx
  ON straylight.task_operation_receipts (user_id, task_id, committed_at DESC)
  WHERE task_id IS NOT NULL AND status = 'committed';

CREATE OR REPLACE FUNCTION straylight.guard_task_operation_receipt()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF TG_OP = 'DELETE' THEN
    RAISE EXCEPTION 'task operation receipts are immutable' USING ERRCODE = '55000';
  END IF;
  IF OLD.status <> 'pending'
     OR NEW.status <> 'committed'
     OR OLD.user_id <> NEW.user_id
     OR OLD.operation_kind <> NEW.operation_kind
     OR OLD.idempotency_key <> NEW.idempotency_key
     OR OLD.request_hash <> NEW.request_hash
     OR OLD.created_by_credential_id <> NEW.created_by_credential_id
     OR OLD.created_at <> NEW.created_at
     OR NEW.receipt IS NULL
     OR NEW.committed_at IS NULL THEN
    RAISE EXCEPTION 'task operation receipts are immutable after one finalization'
      USING ERRCODE = '55000';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER task_operation_receipts_guard
BEFORE UPDATE OR DELETE ON straylight.task_operation_receipts
FOR EACH ROW EXECUTE FUNCTION straylight.guard_task_operation_receipt();

ALTER TABLE straylight.task_operation_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.task_operation_receipts FORCE ROW LEVEL SECURITY;

CREATE POLICY task_operation_receipts_select
ON straylight.task_operation_receipts
FOR SELECT TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write', 'admin'])
);

CREATE POLICY task_operation_receipts_insert
ON straylight.task_operation_receipts
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write', 'admin'])
  AND created_by_credential_id = straylight_auth.current_credential_id()
);

CREATE POLICY task_operation_receipts_finalize
ON straylight.task_operation_receipts
FOR UPDATE TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write', 'admin'])
  AND created_by_credential_id = straylight_auth.current_credential_id()
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write', 'admin'])
  AND created_by_credential_id = straylight_auth.current_credential_id()
);

GRANT SELECT,INSERT,UPDATE ON straylight.task_operation_receipts TO app_rw;

-- Task-read callers may learn only whether the Todoist credential is present.
-- They never receive a secret row, identifier, ciphertext, or access timestamp.
CREATE OR REPLACE FUNCTION straylight.task_todoist_token_configured(p_user_id uuid)
RETURNS boolean
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
BEGIN
  IF NOT straylight_auth.can_access_user(p_user_id)
     OR NOT straylight_auth.has_any_capability(
       ARRAY['task.read', 'task.write', 'integration.manage', 'admin']
     ) THEN
    RAISE EXCEPTION 'task access denied' USING ERRCODE = '42501';
  END IF;
  RETURN EXISTS (
    SELECT 1
    FROM straylight.secrets
    WHERE user_id=p_user_id AND name='todoist-api-token'
  );
END;
$$;

REVOKE ALL ON FUNCTION straylight.task_todoist_token_configured(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight.task_todoist_token_configured(uuid) TO app_rw,app_ro;

-- Checkpoint-only principals may advance activity only after they have linked
-- the same checkpoint to the same registered project. This is deliberately
-- narrower than granting checkpoint credentials generic task-project UPDATE.
CREATE OR REPLACE FUNCTION straylight.touch_task_project_from_checkpoint(
  p_user_id uuid,
  p_checkpoint_entry_id uuid,
  p_project_slug text
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  checkpoint_activity_at timestamptz;
BEGIN
  IF NOT straylight_auth.can_access_user(p_user_id)
     OR NOT straylight_auth.has_any_capability(
       ARRAY['checkpoint', 'task.write', 'admin']
     ) THEN
    RAISE EXCEPTION 'checkpoint project activity access denied' USING ERRCODE='42501';
  END IF;
  SELECT version.created_at
  INTO checkpoint_activity_at
  FROM straylight.task_checkpoint_links AS link
  JOIN straylight.entries AS entry
    ON entry.user_id=link.user_id AND entry.id=link.checkpoint_entry_id
  JOIN straylight.entry_versions AS version
    ON version.user_id=entry.user_id
   AND version.entry_id=entry.id
   AND version.version=entry.current_version
  WHERE link.user_id=p_user_id
    AND link.checkpoint_entry_id=p_checkpoint_entry_id
    AND link.project_slug=p_project_slug
    AND COALESCE(version.metadata->'client',version.metadata)->>'kind'='checkpoint';
  IF checkpoint_activity_at IS NULL THEN
    RAISE EXCEPTION 'checkpoint project link is required' USING ERRCODE='23514';
  END IF;
  UPDATE straylight.task_projects
  SET last_activity_at=GREATEST(last_activity_at,checkpoint_activity_at),
      updated_at=GREATEST(updated_at,checkpoint_activity_at)
  WHERE user_id=p_user_id AND slug=p_project_slug;
END;
$$;

REVOKE ALL ON FUNCTION straylight.touch_task_project_from_checkpoint(uuid,uuid,text)
  FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight.touch_task_project_from_checkpoint(uuid,uuid,text)
  TO app_rw;

CREATE OR REPLACE FUNCTION straylight.is_managed_task_entry(
  p_user_id uuid,
  p_entry_id uuid
)
RETURNS boolean
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
BEGIN
  IF NOT straylight_auth.can_access_user(p_user_id) THEN
    RETURN false;
  END IF;
  RETURN EXISTS (
    SELECT 1 FROM straylight.entries
    WHERE user_id=p_user_id AND id=p_entry_id
      AND path ~ '^\.straylight/tasks/'
  );
END;
$$;
REVOKE ALL ON FUNCTION straylight.is_managed_task_entry(uuid,uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight.is_managed_task_entry(uuid,uuid) TO app_rw,app_ro;

-- Canonical task rows are never writable through the old broad Save policy.
-- Only the exact lowercase UUIDv7 task policy may mutate them.
DROP POLICY workspace_entries_select ON straylight.entries;
CREATE POLICY workspace_entries_select ON straylight.entries
FOR SELECT TO app_rw,app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY[
    'open','query','read','compute','verify','status','checkpoint','save','stage',
    'correct','delete','dream','credential:manage','admin'
  ])
  AND path !~ '^\.straylight/tasks/'
);

DROP POLICY workspace_entry_versions_select ON straylight.entry_versions;
CREATE POLICY workspace_entry_versions_select ON straylight.entry_versions
FOR SELECT TO app_rw,app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY[
    'open','query','read','compute','verify','status','checkpoint','save','stage',
    'correct','delete','dream','credential:manage','admin'
  ])
  AND NOT straylight.is_managed_task_entry(user_id,entry_id)
);

DROP POLICY simple_user_select ON straylight.workspace_changes;
CREATE POLICY workspace_changes_select ON straylight.workspace_changes
FOR SELECT TO app_rw,app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY[
    'open','query','read','compute','verify','status','checkpoint','save','stage',
    'correct','delete','dream','credential:manage','admin'
  ])
  AND path !~ '^\.straylight/tasks/'
);
CREATE POLICY task_workspace_changes_select ON straylight.workspace_changes
FOR SELECT TO app_rw,app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.read','task.write','admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.md$'
);

DROP POLICY simple_user_write ON straylight.entries;
CREATE POLICY simple_user_write ON straylight.entries
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['save','checkpoint','stage','dream','delete']
  )
  AND path !~ '^\.straylight/tasks/'
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['save','checkpoint','stage','dream','delete']
  )
  AND path !~ '^\.straylight/tasks/'
);

DROP POLICY simple_user_write ON straylight.entry_versions;
CREATE POLICY simple_user_write ON straylight.entry_versions
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['save','checkpoint','stage','dream','delete']
  )
  AND NOT straylight.is_managed_task_entry(user_id,entry_id)
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['save','checkpoint','stage','dream','delete']
  )
  AND NOT straylight.is_managed_task_entry(user_id,entry_id)
);

DROP POLICY simple_user_write ON straylight.workspace_changes;
CREATE POLICY simple_user_write ON straylight.workspace_changes
FOR ALL TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['save','checkpoint','stage','dream','delete']
  )
  AND path !~ '^\.straylight/tasks/'
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(
    ARRAY['save','checkpoint','stage','dream','delete']
  )
  AND path !~ '^\.straylight/tasks/'
);

DROP POLICY task_entries_select ON straylight.entries;
CREATE POLICY task_entries_select ON straylight.entries
FOR SELECT TO app_rw,app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.read','task.write','admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.md$'
);

DROP POLICY task_entry_versions_select ON straylight.entry_versions;
CREATE POLICY task_entry_versions_select ON straylight.entry_versions
FOR SELECT TO app_rw,app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.read','task.write','admin'])
  AND EXISTS (
    SELECT 1 FROM straylight.entries AS entry
    WHERE entry.user_id=entry_versions.user_id
      AND entry.id=entry_versions.entry_id
      AND entry.path ~ '^\.straylight/tasks/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.md$'
  )
);

DROP POLICY task_entry_versions_insert ON straylight.entry_versions;
CREATE POLICY task_entry_versions_insert ON straylight.entry_versions
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write','admin'])
  AND EXISTS (
    SELECT 1 FROM straylight.entries AS entry
    WHERE entry.user_id=entry_versions.user_id
      AND entry.id=entry_versions.entry_id
      AND entry.path ~ '^\.straylight/tasks/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.md$'
  )
);

DROP POLICY task_entries_insert ON straylight.entries;
CREATE POLICY task_entries_insert ON straylight.entries
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write','admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.md$'
);

DROP POLICY task_entries_update ON straylight.entries;
CREATE POLICY task_entries_update ON straylight.entries
FOR UPDATE TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write','admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.md$'
)
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write','admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.md$'
);

DROP POLICY task_workspace_changes_insert ON straylight.workspace_changes;
CREATE POLICY task_workspace_changes_insert ON straylight.workspace_changes
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.write','admin'])
  AND path ~ '^\.straylight/tasks/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.md$'
  AND EXISTS (
    SELECT 1 FROM straylight.entries AS entry
    WHERE entry.user_id=workspace_changes.user_id
      AND entry.id=workspace_changes.entry_id
      AND entry.path=workspace_changes.path
  )
);

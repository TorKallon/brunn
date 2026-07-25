-- Fence physical storage writes as soon as account deletion begins, and retain
-- enough intent state to reconcile multipart uploads created before their
-- canonical database row commits.

CREATE TABLE straylight.account_deletion_fences (
  user_id uuid PRIMARY KEY REFERENCES straylight.users(id),
  request_id uuid NOT NULL,
  fenced_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  FOREIGN KEY (user_id, request_id)
    REFERENCES straylight.account_deletion_requests(user_id, id)
);

INSERT INTO straylight.account_deletion_fences (user_id, request_id, fenced_at)
SELECT DISTINCT ON (request.user_id)
       request.user_id,request.id,
       coalesce(request.started_at,request.created_at,clock_timestamp())
FROM straylight.account_deletion_requests AS request
JOIN straylight.users AS user_row ON user_row.id=request.user_id
WHERE user_row.account_status IN ('deleting','deleted')
ORDER BY request.user_id,request.created_at DESC,request.id DESC
ON CONFLICT (user_id) DO NOTHING;

CREATE TRIGGER account_deletion_fences_immutable
BEFORE UPDATE OR DELETE ON straylight.account_deletion_fences
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

CREATE FUNCTION straylight.install_account_deletion_fence()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
DECLARE
  active_request_id uuid;
BEGIN
  IF OLD.account_status = 'active' AND NEW.account_status = 'deleting' THEN
    PERFORM pg_advisory_xact_lock(
      hashtextextended('storage:' || NEW.id::text, 0)
    );

    SELECT request.id
    INTO active_request_id
    FROM straylight.account_deletion_requests AS request
    WHERE request.user_id = NEW.id
      AND request.status IN ('queued','running','awaiting_backup_expiry')
    ORDER BY request.created_at DESC,request.id DESC
    LIMIT 1;

    IF active_request_id IS NULL THEN
      RAISE EXCEPTION 'account deletion requires an active deletion request'
        USING ERRCODE = '55000';
    END IF;

    INSERT INTO straylight.account_deletion_fences (
      user_id,request_id,fenced_at
    ) VALUES (
      NEW.id,active_request_id,clock_timestamp()
    )
    ON CONFLICT (user_id) DO NOTHING;
  ELSIF OLD.account_status IN ('deleting','deleted')
        AND NEW.account_status = 'active' THEN
    RAISE EXCEPTION 'a deletion-fenced account cannot be reactivated'
      USING ERRCODE = '55000';
  END IF;

  RETURN NEW;
END;
$$;

CREATE TRIGGER users_install_account_deletion_fence
BEFORE UPDATE OF account_status ON straylight.users
FOR EACH ROW EXECUTE FUNCTION straylight.install_account_deletion_fence();

CREATE FUNCTION straylight.assert_storage_write_allowed(p_user_id uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
DECLARE
  current_status text;
BEGIN
  PERFORM pg_advisory_xact_lock(
    hashtextextended('storage:' || p_user_id::text, 0)
  );

  SELECT account_status
  INTO current_status
  FROM straylight.users
  WHERE id = p_user_id;

  IF current_status IS NULL THEN
    RAISE EXCEPTION 'storage owner not found' USING ERRCODE = 'P0002';
  END IF;
  IF current_status <> 'active'
     OR EXISTS (
       SELECT 1
       FROM straylight.account_deletion_fences AS fence
       WHERE fence.user_id = p_user_id
     ) THEN
    RAISE EXCEPTION 'account deletion fence is active'
      USING ERRCODE = '55000';
  END IF;
END;
$$;

REVOKE ALL ON FUNCTION straylight.assert_storage_write_allowed(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight.assert_storage_write_allowed(uuid) TO app_rw;

CREATE TABLE straylight.asset_multipart_intents (
  id uuid PRIMARY KEY,
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  credential_id uuid NOT NULL,
  object_key text NOT NULL,
  multipart_upload_id text,
  expires_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, object_key),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  CHECK (object_key = user_id::text || '/uploads/' || id::text),
  CHECK (multipart_upload_id IS NULL OR btrim(multipart_upload_id) <> ''),
  CHECK (expires_at > created_at)
);

CREATE INDEX asset_multipart_intents_expiry_idx
  ON straylight.asset_multipart_intents (expires_at,id);

ALTER TABLE straylight.asset_multipart_intents ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.asset_multipart_intents FORCE ROW LEVEL SECURITY;

CREATE POLICY asset_multipart_intents_select
  ON straylight.asset_multipart_intents
  FOR SELECT TO app_rw
  USING (straylight_auth.can_access_scope(user_id, scope_id));

CREATE POLICY asset_multipart_intents_write
  ON straylight.asset_multipart_intents
  FOR ALL TO app_rw
  USING (
    straylight_auth.can_access_scope(user_id, scope_id)
    AND straylight_auth.has_any_capability(ARRAY['stage'])
  )
  WITH CHECK (
    straylight_auth.can_access_scope(user_id, scope_id)
    AND straylight_auth.has_any_capability(ARRAY['stage'])
  );

GRANT SELECT,INSERT,UPDATE,DELETE
  ON straylight.asset_multipart_intents TO app_rw;

COMMENT ON TABLE straylight.account_deletion_fences IS
  'Immutable storage-write fence retained with the minimal account-deletion receipt.';
COMMENT ON TABLE straylight.asset_multipart_intents IS
  'Short-lived durable intents used to reconcile S3 multipart uploads that crash before asset_uploads commit.';

-- The deletion fence is a retained receipt. Every other user-owned table,
-- including multipart intents, remains subject to canonical account purge.
CREATE OR REPLACE FUNCTION straylight.purge_account_user_rows(p_user_id uuid)
RETURNS jsonb
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight
SET row_security = off
AS $$
DECLARE
  target_table record;
  deleted_rows bigint;
  remaining_rows bigint;
  result jsonb := '{}'::jsonb;
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM straylight.users
    WHERE id = p_user_id AND account_status = 'deleting'
  ) THEN
    RAISE EXCEPTION 'account purge requires a deleting user'
      USING ERRCODE = '55000';
  END IF;
  IF NOT EXISTS (
    SELECT 1 FROM straylight.account_deletion_fences
    WHERE user_id = p_user_id
  ) THEN
    RAISE EXCEPTION 'account purge requires a durable deletion fence'
      USING ERRCODE = '55000';
  END IF;

  PERFORM set_config('session_replication_role', 'replica', true);
  FOR target_table IN
    SELECT column_row.table_name
    FROM information_schema.columns AS column_row
    JOIN information_schema.tables AS schema_table
      ON schema_table.table_schema = column_row.table_schema
     AND schema_table.table_name = column_row.table_name
     AND schema_table.table_type = 'BASE TABLE'
    WHERE column_row.table_schema = 'straylight'
      AND column_row.column_name = 'user_id'
      AND column_row.table_name <> ALL(ARRAY[
        'users', 'api_credentials', 'account_deletion_requests',
        'account_deletion_fences'
      ]::text[])
    ORDER BY column_row.table_name
  LOOP
    EXECUTE format(
      'DELETE FROM straylight.%I WHERE user_id=$1',
      target_table.table_name
    ) USING p_user_id;
    GET DIAGNOSTICS deleted_rows = ROW_COUNT;
    result := result || jsonb_build_object(target_table.table_name, deleted_rows);
  END LOOP;
  PERFORM set_config('session_replication_role', 'origin', true);

  FOR target_table IN
    SELECT column_row.table_name
    FROM information_schema.columns AS column_row
    JOIN information_schema.tables AS schema_table
      ON schema_table.table_schema = column_row.table_schema
     AND schema_table.table_name = column_row.table_name
     AND schema_table.table_type = 'BASE TABLE'
    WHERE column_row.table_schema = 'straylight'
      AND column_row.column_name = 'user_id'
      AND column_row.table_name <> ALL(ARRAY[
        'users', 'api_credentials', 'account_deletion_requests',
        'account_deletion_fences'
      ]::text[])
    ORDER BY column_row.table_name
  LOOP
    EXECUTE format(
      'SELECT count(*) FROM straylight.%I WHERE user_id=$1',
      target_table.table_name
    ) INTO remaining_rows USING p_user_id;
    IF remaining_rows <> 0 THEN
      RAISE EXCEPTION '% rows remain in %.% after account purge',
        remaining_rows, 'straylight', target_table.table_name
        USING ERRCODE = '55000';
    END IF;
  END LOOP;

  RETURN result;
EXCEPTION WHEN OTHERS THEN
  PERFORM set_config('session_replication_role', 'origin', true);
  RAISE;
END;
$$;

REVOKE ALL ON FUNCTION straylight.purge_account_user_rows(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight.purge_account_user_rows(uuid) FROM app_rw, app_ro;

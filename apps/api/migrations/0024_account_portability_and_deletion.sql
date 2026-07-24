CREATE UNIQUE INDEX account_exports_one_active_idx
  ON straylight.account_exports (user_id)
  WHERE status IN ('queued', 'running', 'ready');

ALTER TABLE straylight.account_exports
  ADD COLUMN attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  ADD COLUMN max_attempts integer NOT NULL DEFAULT 5 CHECK (max_attempts > 0),
  ADD COLUMN available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  ADD COLUMN locked_at timestamptz,
  ADD COLUMN locked_by text,
  ADD CONSTRAINT account_exports_attempts_within_max_check
    CHECK (attempts <= max_attempts);

ALTER TABLE straylight.account_deletion_requests
  ADD COLUMN failure_attempts integer NOT NULL DEFAULT 0 CHECK (failure_attempts >= 0),
  ADD COLUMN max_failure_attempts integer NOT NULL DEFAULT 5 CHECK (max_failure_attempts > 0),
  ADD COLUMN locked_at timestamptz,
  ADD COLUMN locked_by text,
  ADD CONSTRAINT account_deletion_failure_attempts_check
    CHECK (failure_attempts <= max_failure_attempts);

CREATE TABLE straylight.account_deletion_targets (
  user_id uuid NOT NULL,
  request_id uuid NOT NULL,
  record_id uuid NOT NULL,
  deletion_job_id uuid NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, request_id, record_id),
  FOREIGN KEY (user_id, request_id)
    REFERENCES straylight.account_deletion_requests(user_id, id),
  FOREIGN KEY (user_id, record_id)
    REFERENCES straylight.record_keys(user_id, record_id),
  FOREIGN KEY (user_id, deletion_job_id)
    REFERENCES straylight.deletion_jobs(user_id, id)
);

CREATE INDEX account_deletion_targets_job_idx
  ON straylight.account_deletion_targets (user_id, request_id, deletion_job_id);

ALTER TABLE straylight.account_deletion_targets ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.account_deletion_targets FORCE ROW LEVEL SECURITY;

CREATE POLICY account_deletion_targets_self_select
  ON straylight.account_deletion_targets
  FOR SELECT TO app_rw, app_ro
  USING (straylight_auth.can_access_user(user_id));

GRANT SELECT ON straylight.account_deletion_targets TO app_rw, app_ro;

CREATE FUNCTION straylight.guard_account_deletion_redaction()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
AS $$
DECLARE
  request_setting text := current_setting(
    'straylight.account_deletion_request_id', true
  );
  deletion_job_setting text := current_setting(
    'straylight.deletion_job_id', true
  );
  request_id uuid;
  deletion_job_id uuid;
  row_user_id uuid;
  old_shape jsonb := to_jsonb(OLD);
  new_shape jsonb := to_jsonb(NEW);
  argument_index integer;
  administrator boolean := false;
  authorized boolean := false;
BEGIN
  IF TG_OP <> 'UPDATE' THEN
    RAISE EXCEPTION '% is immutable; only redaction updates are allowed', TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;

  SELECT role.rolsuper
      OR pg_has_role(current_user, pg_get_userbyid(database.datdba), 'MEMBER')
  INTO administrator
  FROM pg_roles AS role
  JOIN pg_database AS database ON database.datname = current_database()
  WHERE role.rolname = current_user;

  BEGIN
    request_id := nullif(request_setting, '')::uuid;
    deletion_job_id := nullif(deletion_job_setting, '')::uuid;
    row_user_id := nullif(to_jsonb(OLD) ->> 'user_id', '')::uuid;
  EXCEPTION WHEN invalid_text_representation THEN
    request_id := NULL;
    deletion_job_id := NULL;
    row_user_id := NULL;
  END;

  IF request_id IS NOT NULL AND row_user_id IS NOT NULL THEN
    authorized := EXISTS (
      SELECT 1
      FROM straylight.account_deletion_requests AS request
      WHERE request.id = request_id
        AND request.user_id = row_user_id
        AND request.status IN ('running', 'awaiting_backup_expiry')
    );
  END IF;
  IF NOT authorized AND deletion_job_id IS NOT NULL AND row_user_id IS NOT NULL THEN
    authorized := EXISTS (
      SELECT 1
      FROM straylight.deletion_jobs AS job
      WHERE job.id = deletion_job_id
        AND job.user_id = row_user_id
        AND job.status = 'propagating'
    );
  END IF;

  IF NOT coalesce(administrator, false) OR NOT authorized THEN
    RAISE EXCEPTION '% is immutable outside an authorized deletion', TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;

  IF TG_NARGS = 0 THEN
    RAISE EXCEPTION 'no redaction columns are configured for %', TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;
  FOR argument_index IN 0..TG_NARGS - 1 LOOP
    old_shape := old_shape - TG_ARGV[argument_index];
    new_shape := new_shape - TG_ARGV[argument_index];
  END LOOP;
  IF old_shape IS DISTINCT FROM new_shape THEN
    RAISE EXCEPTION 'deletion redaction attempted to mutate protected columns on %',
      TG_TABLE_NAME USING ERRCODE = '55000';
  END IF;
  RETURN NEW;
END;
$$;

DROP TRIGGER audit_events_immutable ON straylight.audit_events;
CREATE TRIGGER audit_events_deletion_redaction
BEFORE UPDATE OR DELETE ON straylight.audit_events
FOR EACH ROW EXECUTE FUNCTION straylight.guard_account_deletion_redaction(
  'actor_ref', 'request_id', 'details', 'content_free'
);

CREATE FUNCTION straylight_auth.request_account_deletion(
  p_user_id uuid,
  p_confirmation text,
  p_reason text,
  p_backup_retention_days integer
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  expected_confirmation text;
  request_id uuid := gen_random_uuid();
BEGIN
  PERFORM straylight_auth.require_credential_control(p_user_id);

  SELECT 'DELETE ' || external_ref
  INTO expected_confirmation
  FROM straylight.users
  WHERE id = p_user_id AND account_status = 'active'
  FOR UPDATE;

  IF expected_confirmation IS NULL THEN
    RAISE EXCEPTION 'active user not found' USING ERRCODE = 'P0002';
  END IF;
  IF p_confirmation IS DISTINCT FROM expected_confirmation THEN
    RAISE EXCEPTION 'account deletion confirmation does not match'
      USING ERRCODE = '22023';
  END IF;
  IF p_reason IS NULL OR btrim(p_reason) = '' OR length(p_reason) > 500 THEN
    RAISE EXCEPTION 'account deletion reason must contain 1 to 500 characters'
      USING ERRCODE = '22023';
  END IF;
  IF p_backup_retention_days < 1 OR p_backup_retention_days > 90 THEN
    RAISE EXCEPTION 'backup retention must be between 1 and 90 days'
      USING ERRCODE = '22023';
  END IF;
  IF EXISTS (
    SELECT 1
    FROM straylight.account_exports
    WHERE user_id = p_user_id AND status = 'running'
  ) THEN
    RAISE EXCEPTION 'a running account export must finish before deletion'
      USING ERRCODE = '55000';
  END IF;

  INSERT INTO straylight.account_deletion_requests (
    id, user_id, requested_by_credential_id, status, confirmation_hash,
    reason, backup_expiry_due_at
  ) VALUES (
    request_id,
    p_user_id,
    straylight_auth.current_credential_id(),
    'queued',
    encode(public.digest(p_confirmation, 'sha256'), 'hex'),
    p_reason,
    clock_timestamp() + make_interval(days => p_backup_retention_days)
  );

  UPDATE straylight.users
  SET account_status = 'deleting',
      deletion_requested_at = clock_timestamp()
  WHERE id = p_user_id;

  UPDATE straylight.account_exports
  SET status = 'deleted',
      completed_at = coalesce(completed_at, clock_timestamp())
  WHERE user_id = p_user_id AND status IN ('queued', 'ready');

  INSERT INTO straylight.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    straylight_auth.current_credential_id(),
    'account.deletion.request',
    jsonb_build_object(
      'request_id', request_id,
      'backup_retention_days', p_backup_retention_days
    ),
    true
  );

  RETURN request_id;
END;
$$;

CREATE FUNCTION straylight_auth.mark_account_export_deleted(
  p_user_id uuid,
  p_export_id uuid
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  current_status text;
BEGIN
  PERFORM straylight_auth.require_credential_control(p_user_id);

  SELECT status INTO current_status
  FROM straylight.account_exports
  WHERE user_id = p_user_id AND id = p_export_id
  FOR UPDATE;
  IF current_status IS NULL THEN
    RAISE EXCEPTION 'account export not found' USING ERRCODE = 'P0002';
  END IF;
  IF current_status = 'running' THEN
    RAISE EXCEPTION 'a running export cannot be deleted'
      USING ERRCODE = '55000';
  END IF;

  UPDATE straylight.account_exports
  SET status = 'deleted',
      object_key = NULL,
      completed_at = coalesce(completed_at, clock_timestamp())
  WHERE user_id = p_user_id AND id = p_export_id;

  INSERT INTO straylight.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    straylight_auth.current_credential_id(),
    'account.export.delete',
    jsonb_build_object('export_id', p_export_id),
    true
  );
END;
$$;

REVOKE ALL ON FUNCTION straylight_auth.request_account_deletion(
  uuid, text, text, integer
) FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight.guard_account_deletion_redaction() FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.mark_account_export_deleted(
  uuid, uuid
) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight_auth.request_account_deletion(
  uuid, text, text, integer
) TO app_rw;
GRANT EXECUTE ON FUNCTION straylight_auth.mark_account_export_deleted(
  uuid, uuid
) TO app_rw;

COMMENT ON TABLE straylight.account_deletion_targets IS
  'Immutable mapping from an account deletion request to per-record deletion jobs.';

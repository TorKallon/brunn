-- Evaluation-only users must be removable without granting their temporary
-- credentials general credential-management authority. The caller may clean
-- up only its own cryptographically derived evaluation identity.

CREATE FUNCTION straylight_auth.cleanup_evaluation_user(
  p_user_id uuid,
  p_credential_id uuid
)
RETURNS TABLE (
  entries_removed bigint,
  search_chunks_removed bigint,
  jobs_removed bigint,
  credentials_revoked bigint,
  revoked_at timestamptz
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  target_external_ref text;
  active_jobs bigint;
  removed_entries bigint;
  removed_chunks bigint;
  removed_jobs bigint;
  revoked_credentials bigint;
  cleanup_time timestamptz := clock_timestamp();
BEGIN
  IF NOT straylight_auth.context_is_valid()
     OR straylight_auth.current_user_id() IS DISTINCT FROM p_user_id
     OR straylight_auth.current_credential_id() IS DISTINCT FROM p_credential_id
     OR NOT straylight_auth.has_capability('delete') THEN
    RAISE EXCEPTION 'evaluation cleanup requires its own delete-capable credential'
      USING ERRCODE = '42501';
  END IF;

  SELECT user_row.external_ref INTO target_external_ref
  FROM straylight.users AS user_row
  WHERE user_row.id = p_user_id;

  IF target_external_ref IS NULL
     OR target_external_ref !~ '^eval-user:[0-9a-f]{64}$' THEN
    RAISE EXCEPTION 'evaluation cleanup cannot target an ordinary user'
      USING ERRCODE = '42501';
  END IF;

  DELETE FROM straylight.jobs AS job
  WHERE job.user_id = p_user_id
    AND job.status <> 'running';
  GET DIAGNOSTICS removed_jobs = ROW_COUNT;

  -- If a worker claimed a queued row concurrently, PostgreSQL rechecks the
  -- DELETE predicate after waiting for that update and leaves the running row.
  -- Refuse the whole transaction rather than racing fixture cleanup.
  SELECT count(*) INTO active_jobs
  FROM straylight.jobs AS job
  WHERE job.user_id = p_user_id;

  IF active_jobs <> 0 THEN
    RAISE EXCEPTION 'evaluation cleanup requires all running jobs to finish'
      USING ERRCODE = '55006';
  END IF;

  DELETE FROM straylight.search_chunks AS chunk
  WHERE chunk.user_id = p_user_id;
  GET DIAGNOSTICS removed_chunks = ROW_COUNT;

  UPDATE straylight.entries AS entry
  SET deleted_at = coalesce(entry.deleted_at, cleanup_time),
      updated_at = cleanup_time
  WHERE entry.user_id = p_user_id
    AND entry.deleted_at IS NULL;
  GET DIAGNOSTICS removed_entries = ROW_COUNT;

  INSERT INTO straylight.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    NULL,
    p_credential_id,
    'evaluation.cleanup',
    jsonb_build_object(
      'entries_removed', removed_entries,
      'search_chunks_removed', removed_chunks,
      'jobs_removed', removed_jobs
    ),
    true
  );

  UPDATE straylight.api_credentials AS credential
  SET disabled_at = coalesce(credential.disabled_at, cleanup_time)
  WHERE credential.user_id = p_user_id
    AND credential.disabled_at IS NULL;
  GET DIAGNOSTICS revoked_credentials = ROW_COUNT;

  RETURN QUERY SELECT
    removed_entries,
    removed_chunks,
    removed_jobs,
    revoked_credentials,
    cleanup_time;
END;
$$;

REVOKE ALL ON FUNCTION straylight_auth.cleanup_evaluation_user(
  uuid, uuid
) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight_auth.cleanup_evaluation_user(
  uuid, uuid
) TO app_rw;

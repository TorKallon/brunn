-- Keep immutable asset history protected while allowing two administrator-only
-- maintenance operations:
--   1. remap provider-issued object version locators during disaster recovery;
--   2. remove unpublished asset rows owned only by an expired stage.
--
-- Both operations retain normal triggers. A transaction-local, row-specific
-- context lets the trigger recognize only the exact row being maintained.

CREATE OR REPLACE FUNCTION brunn.database_administrator()
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY INVOKER
SET search_path = pg_catalog
AS $$
  SELECT coalesce(
    (
      SELECT role.rolsuper
          OR pg_has_role(
            current_user,
            pg_get_userbyid(database.datdba),
            'MEMBER'
          )
      FROM pg_roles AS role
      JOIN pg_database AS database
        ON database.datname=current_database()
      WHERE role.rolname=current_user
    ),
    false
  )
$$;

CREATE OR REPLACE FUNCTION brunn.asset_internal_operation_authorized(
  p_operation text,
  p_user_id uuid,
  p_asset_id uuid,
  p_version integer,
  p_source_version_id text DEFAULT NULL,
  p_restored_version_id text DEFAULT NULL
)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY INVOKER
SET search_path = pg_catalog, brunn
AS $$
  SELECT brunn.database_administrator()
    AND current_setting('brunn.asset_internal_operation', true)
      = p_operation || ':' || encode(
        public.digest(
          jsonb_build_array(
            p_user_id::text,
            p_asset_id::text,
            p_version,
            p_source_version_id,
            p_restored_version_id
          )::text,
          'sha256'
        ),
        'hex'
      )
$$;

REVOKE ALL ON FUNCTION brunn.database_administrator()
FROM PUBLIC,app_rw,app_ro;
REVOKE ALL ON FUNCTION brunn.asset_internal_operation_authorized(
  text,
  uuid,
  uuid,
  integer,
  text,
  text
) FROM PUBLIC,app_rw,app_ro;
GRANT EXECUTE ON FUNCTION brunn.database_administrator()
TO app_rw,app_ro;
GRANT EXECUTE ON FUNCTION brunn.asset_internal_operation_authorized(
  text,
  uuid,
  uuid,
  integer,
  text,
  text
) TO app_rw,app_ro;

-- Migration 0016 replaced the original asset-version immutability trigger with
-- this deletion-redaction guard. Preserve that behavior and add only exact,
-- administrator-authorized locator updates and unpublished-stage deletes.
CREATE OR REPLACE FUNCTION brunn.guard_deletion_redaction()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
AS $$
DECLARE
  deletion_job_setting text := current_setting('brunn.deletion_job_id', true);
  deletion_job_id uuid;
  row_user_id uuid;
  old_shape jsonb := to_jsonb(OLD);
  new_shape jsonb := to_jsonb(NEW);
  argument_index integer;
  administrator boolean := false;
BEGIN
  IF TG_TABLE_SCHEMA='brunn' AND TG_TABLE_NAME='asset_versions' THEN
    IF TG_OP='UPDATE'
       AND brunn.asset_internal_operation_authorized(
         'restore_locator_remap',
         OLD.user_id,
         OLD.asset_id,
         OLD.version,
         OLD.object_version_id,
         NEW.object_version_id
       ) THEN
      IF OLD.object_version_id IS NULL
         OR NEW.object_version_id IS NULL
         OR btrim(NEW.object_version_id) IN ('', 'null') THEN
        RAISE EXCEPTION 'asset recovery requires exact non-null provider version IDs'
          USING ERRCODE = '55000';
      END IF;
      IF (old_shape - 'object_version_id')
           IS DISTINCT FROM (new_shape - 'object_version_id') THEN
        RAISE EXCEPTION 'asset recovery may only remap object_version_id'
          USING ERRCODE = '55000';
      END IF;
      RETURN NEW;
    END IF;

    IF TG_OP='DELETE'
       AND brunn.asset_internal_operation_authorized(
         'stage_reclaim',
         OLD.user_id,
         OLD.asset_id,
         OLD.version
       ) THEN
      RETURN OLD;
    END IF;
  END IF;

  IF TG_OP <> 'UPDATE' THEN
    RAISE EXCEPTION '% is immutable; create a new revision or lineage record', TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;

  SELECT role.rolsuper
      OR pg_has_role(current_user, pg_get_userbyid(database.datdba), 'MEMBER')
  INTO administrator
  FROM pg_roles AS role
  JOIN pg_database AS database ON database.datname = current_database()
  WHERE role.rolname = current_user;

  BEGIN
    deletion_job_id := nullif(deletion_job_setting, '')::uuid;
    row_user_id := nullif(to_jsonb(OLD) ->> 'user_id', '')::uuid;
  EXCEPTION WHEN invalid_text_representation THEN
    deletion_job_id := NULL;
    row_user_id := NULL;
  END;

  IF NOT coalesce(administrator, false)
     OR deletion_job_id IS NULL
     OR row_user_id IS NULL
     OR NOT EXISTS (
       SELECT 1
       FROM brunn.deletion_jobs AS job
       WHERE job.id = deletion_job_id
         AND job.user_id = row_user_id
         AND job.status = 'propagating'
     ) THEN
    RAISE EXCEPTION '% is immutable outside an administrator deletion job', TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;

  IF TG_NARGS = 0 THEN
    RAISE EXCEPTION 'no redaction columns are configured for %', TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;

  FOR argument_index IN 0..TG_NARGS - 1 LOOP
    IF NOT old_shape ? TG_ARGV[argument_index] THEN
      RAISE EXCEPTION 'unknown redaction column %.%', TG_TABLE_NAME, TG_ARGV[argument_index]
        USING ERRCODE = '42703';
    END IF;
    old_shape := old_shape - TG_ARGV[argument_index];
    new_shape := new_shape - TG_ARGV[argument_index];
  END LOOP;

  IF old_shape IS DISTINCT FROM new_shape THEN
    RAISE EXCEPTION 'deletion redaction attempted to mutate protected columns on %', TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;

  RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION brunn.enforce_sequential_head_advance()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF TG_TABLE_SCHEMA='brunn'
     AND TG_TABLE_NAME='assets'
     AND brunn.asset_internal_operation_authorized(
       'stage_reclaim',
       OLD.user_id,
       OLD.id,
       OLD.current_version
     ) THEN
    IF NEW.current_version <> OLD.current_version - 1
       OR NEW.current_version < 1 THEN
      RAISE EXCEPTION
        'stage reclamation may only roll an asset head back one version'
        USING ERRCODE = '55000';
    END IF;
    RETURN NEW;
  END IF;

  IF NEW.current_version <> OLD.current_version + 1 THEN
    RAISE EXCEPTION 'current_version must advance exactly once (old %, new %)',
      OLD.current_version, NEW.current_version
      USING ERRCODE = '40001';
  END IF;
  RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION brunn.prevent_physical_delete()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF TG_TABLE_SCHEMA='brunn' AND TG_TABLE_NAME='assets' THEN
    IF brunn.asset_internal_operation_authorized(
      'stage_reclaim',
      OLD.user_id,
      OLD.id,
      OLD.current_version
    ) THEN
      RETURN OLD;
    END IF;
  END IF;

  RAISE EXCEPTION '% cannot be physically deleted; create a tombstone', TG_TABLE_NAME
    USING ERRCODE = '55000';
END;
$$;

CREATE OR REPLACE FUNCTION brunn.remap_asset_object_versions(p_mapping jsonb)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  candidate record;
  updated_count bigint := 0;
  affected bigint;
  operation_context text;
BEGIN
  IF NOT brunn.database_administrator() THEN
    RAISE EXCEPTION 'asset locator recovery requires a database administrator'
      USING ERRCODE = '42501';
  END IF;
  IF p_mapping IS NULL OR jsonb_typeof(p_mapping) <> 'array' THEN
    RAISE EXCEPTION 'asset locator recovery mapping must be a JSON array'
      USING ERRCODE = '22023';
  END IF;

  CREATE TEMP TABLE pg_temp.asset_object_version_recovery_map (
    object_key text NOT NULL,
    source_version_id text NOT NULL,
    restored_version_id text NOT NULL,
    content_hash text NOT NULL,
    size_bytes bigint NOT NULL,
    PRIMARY KEY (object_key,source_version_id),
    UNIQUE (object_key,restored_version_id),
    CHECK (length(btrim(object_key)) > 0),
    CHECK (length(btrim(source_version_id)) > 0 AND source_version_id <> 'null'),
    CHECK (length(btrim(restored_version_id)) > 0 AND restored_version_id <> 'null'),
    CHECK (content_hash ~ '^[0-9a-f]{64}$'),
    CHECK (size_bytes >= 0)
  ) ON COMMIT DROP;

  INSERT INTO pg_temp.asset_object_version_recovery_map (
    object_key,
    source_version_id,
    restored_version_id,
    content_hash,
    size_bytes
  )
  SELECT item.object_key,
         item.source_version_id,
         item.restored_version_id,
         item.content_hash,
         item.size_bytes
  FROM jsonb_to_recordset(p_mapping) AS item(
    object_key text,
    source_version_id text,
    restored_version_id text,
    content_hash text,
    size_bytes bigint
  );

  IF EXISTS (
    SELECT 1
    FROM (
      SELECT object_key,source_version_id AS version_id
      FROM pg_temp.asset_object_version_recovery_map
      UNION ALL
      SELECT object_key,restored_version_id
      FROM pg_temp.asset_object_version_recovery_map
    ) AS identifiers
    GROUP BY object_key,version_id
    HAVING count(*) > 1
  ) THEN
    RAISE EXCEPTION 'asset locator recovery mapping contains ambiguous source or target IDs'
      USING ERRCODE = '22023';
  END IF;

  -- Hold every candidate parent row through validation and remapping. This also
  -- serializes a repeated recovery invocation against the same restored rows.
  PERFORM 1
  FROM brunn.asset_versions AS version
  WHERE version.object_version_id IS NOT NULL
  ORDER BY version.user_id,version.asset_id,version.version
  FOR UPDATE;

  IF EXISTS (
    SELECT 1
    FROM brunn.asset_versions AS version
    LEFT JOIN pg_temp.asset_object_version_recovery_map AS mapping
      ON mapping.object_key=version.object_key
     AND version.object_version_id IN (
       mapping.source_version_id,
       mapping.restored_version_id
     )
    WHERE version.object_version_id IS NOT NULL
      AND (
        mapping.object_key IS NULL
        OR mapping.content_hash<>version.content_hash
        OR mapping.size_bytes<>version.size_bytes
      )
  ) THEN
    RAISE EXCEPTION
      'asset locator recovery mapping is incomplete or changes key/hash/size identity'
      USING ERRCODE = '22023';
  END IF;

  FOR candidate IN
    SELECT version.user_id,
           version.asset_id,
           version.version,
           version.object_key,
           version.content_hash,
           version.size_bytes,
           version.object_version_id AS source_version_id,
           mapping.restored_version_id
    FROM brunn.asset_versions AS version
    JOIN pg_temp.asset_object_version_recovery_map AS mapping
      ON mapping.object_key=version.object_key
     AND mapping.source_version_id=version.object_version_id
     AND mapping.content_hash=version.content_hash
     AND mapping.size_bytes=version.size_bytes
    ORDER BY version.user_id,version.asset_id,version.version
  LOOP
    operation_context := 'restore_locator_remap:' || encode(
      public.digest(
        jsonb_build_array(
          candidate.user_id::text,
          candidate.asset_id::text,
          candidate.version,
          candidate.source_version_id,
          candidate.restored_version_id
        )::text,
        'sha256'
      ),
      'hex'
    );
    PERFORM set_config(
      'brunn.asset_internal_operation',
      operation_context,
      true
    );

    UPDATE brunn.asset_versions AS version
    SET object_version_id=candidate.restored_version_id
    WHERE version.user_id=candidate.user_id
      AND version.asset_id=candidate.asset_id
      AND version.version=candidate.version
      AND version.object_key=candidate.object_key
      AND version.content_hash=candidate.content_hash
      AND version.size_bytes=candidate.size_bytes
      AND version.object_version_id=candidate.source_version_id;
    GET DIAGNOSTICS affected = ROW_COUNT;
    IF affected <> 1 THEN
      RAISE EXCEPTION 'asset locator changed during recovery'
        USING ERRCODE = '40001';
    END IF;
    updated_count := updated_count + affected;
    PERFORM set_config('brunn.asset_internal_operation','',true);
  END LOOP;

  IF EXISTS (
    SELECT 1
    FROM brunn.asset_versions AS version
    LEFT JOIN pg_temp.asset_object_version_recovery_map AS mapping
      ON mapping.object_key=version.object_key
     AND mapping.restored_version_id=version.object_version_id
     AND mapping.content_hash=version.content_hash
     AND mapping.size_bytes=version.size_bytes
    WHERE version.object_version_id IS NOT NULL
      AND mapping.object_key IS NULL
  ) THEN
    RAISE EXCEPTION
      'asset locator recovery did not produce a complete identity-preserving remap'
      USING ERRCODE = '40001';
  END IF;

  PERFORM set_config('brunn.asset_internal_operation','',true);
  RETURN updated_count;
EXCEPTION WHEN OTHERS THEN
  PERFORM set_config('brunn.asset_internal_operation','',true);
  RAISE;
END;
$$;

REVOKE ALL ON FUNCTION brunn.remap_asset_object_versions(jsonb)
FROM PUBLIC,app_rw,app_ro;

-- Replace migration 0039's trigger-suppressed implementation. Candidate
-- asset/version/record-key parent rows are locked before reference discovery.
-- A concurrent child insert must therefore wait for this transaction and will
-- fail its foreign key if reclamation commits.
CREATE OR REPLACE FUNCTION brunn.expire_unpromoted_stage(p_stage_id uuid)
RETURNS TABLE(reclaimed_object_key text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  target_stage record;
  candidate_ref record;
  candidate record;
  reclaimed_keys text[] := ARRAY[]::text[];
  operation_context text;
  affected bigint;
BEGIN
  SELECT stage.id,stage.user_id,stage.scope_id,stage.status,stage.expires_at
  INTO target_stage
  FROM brunn.stages AS stage
  WHERE stage.id=p_stage_id;

  IF NOT FOUND THEN
    RETURN;
  END IF;

  PERFORM pg_advisory_xact_lock(
    hashtextextended('storage:' || target_stage.user_id::text,0)
  );

  SELECT stage.id,stage.user_id,stage.scope_id,stage.status,stage.expires_at
  INTO target_stage
  FROM brunn.stages AS stage
  WHERE stage.id=p_stage_id
  FOR UPDATE;

  IF NOT FOUND
     OR target_stage.status='promoted'
     OR (
       target_stage.expires_at > clock_timestamp()
       AND target_stage.status NOT IN ('failed','quarantined')
     ) THEN
    RETURN;
  END IF;

  UPDATE brunn.stages
  SET status='expired'
  WHERE id=p_stage_id
    AND status IN ('uploading','inspecting','ready','quarantined','failed');

  UPDATE brunn.background_jobs
  SET status='canceled',completed_at=clock_timestamp(),
      result=jsonb_build_object('reason','stage_expired'),
      locked_at=NULL,locked_by=NULL
  WHERE user_id=target_stage.user_id
    AND payload->>'stage_id'=p_stage_id::text
    AND status IN ('queued','running','retry_wait');

  <<candidate_loop>>
  FOR candidate_ref IN
    SELECT DISTINCT entry.asset_id,entry.asset_version
    FROM brunn.staged_entries AS entry
    WHERE entry.user_id=target_stage.user_id
      AND entry.stage_id=p_stage_id
      AND entry.asset_id IS NOT NULL
    ORDER BY entry.asset_id,entry.asset_version
  LOOP
    -- These parent locks fence every concurrent foreign-key insertion.
    SELECT version.asset_id,
           version.version,
           version.previous_version,
           version.object_key,
           asset.current_version
    INTO candidate
    FROM brunn.asset_versions AS version
    JOIN brunn.assets AS asset
      ON asset.user_id=version.user_id
     AND asset.id=version.asset_id
    WHERE version.user_id=target_stage.user_id
      AND version.asset_id=candidate_ref.asset_id
      AND version.version=candidate_ref.asset_version
    FOR UPDATE OF version,asset;

    IF NOT FOUND OR candidate.current_version<>candidate.version THEN
      CONTINUE candidate_loop;
    END IF;

    IF candidate.previous_version IS NULL THEN
      PERFORM 1
      FROM brunn.record_keys AS record_key
      WHERE record_key.user_id=target_stage.user_id
        AND record_key.record_id=candidate.asset_id
        AND record_key.record_kind='asset'
      FOR UPDATE;
      IF NOT FOUND THEN
        CONTINUE candidate_loop;
      END IF;
    END IF;

    DELETE FROM brunn.staged_entries
    WHERE user_id=target_stage.user_id
      AND stage_id=p_stage_id
      AND asset_id=candidate.asset_id
      AND asset_version=candidate.version;

    IF brunn.foreign_key_reference_exists(
      'brunn.asset_versions'::regclass,
      jsonb_build_object(
        'user_id',target_stage.user_id::text,
        'asset_id',candidate.asset_id::text,
        'version',candidate.version::text
      ),
      ARRAY['brunn.assets'::regclass]
    ) THEN
      CONTINUE candidate_loop;
    END IF;

    IF candidate.previous_version IS NULL
       AND (
         EXISTS (
           SELECT 1
           FROM brunn.asset_versions AS other_version
           WHERE other_version.user_id=target_stage.user_id
             AND other_version.asset_id=candidate.asset_id
             AND other_version.version<>candidate.version
         )
         OR brunn.foreign_key_reference_exists(
           'brunn.assets'::regclass,
           jsonb_build_object(
             'user_id',target_stage.user_id::text,
             'id',candidate.asset_id::text
           ),
           ARRAY['brunn.asset_versions'::regclass]
         )
         OR brunn.foreign_key_reference_exists(
           'brunn.record_keys'::regclass,
           jsonb_build_object(
             'user_id',target_stage.user_id::text,
             'record_id',candidate.asset_id::text,
             'record_kind','asset'
           )
         )
       ) THEN
      CONTINUE candidate_loop;
    END IF;

    operation_context := 'stage_reclaim:' || encode(
      public.digest(
        jsonb_build_array(
          target_stage.user_id::text,
          candidate.asset_id::text,
          candidate.version,
          NULL,
          NULL
        )::text,
        'sha256'
      ),
      'hex'
    );
    PERFORM set_config(
      'brunn.asset_internal_operation',
      operation_context,
      true
    );

    IF candidate.previous_version IS NULL THEN
      DELETE FROM brunn.asset_versions
      WHERE user_id=target_stage.user_id
        AND asset_id=candidate.asset_id
        AND version=candidate.version;
      GET DIAGNOSTICS affected = ROW_COUNT;
      IF affected <> 1 THEN
        RAISE EXCEPTION 'unpublished asset version changed during reclamation'
          USING ERRCODE = '40001';
      END IF;

      DELETE FROM brunn.assets
      WHERE user_id=target_stage.user_id
        AND id=candidate.asset_id
        AND current_version=candidate.version;
      GET DIAGNOSTICS affected = ROW_COUNT;
      IF affected <> 1 THEN
        RAISE EXCEPTION 'unpublished asset changed during reclamation'
          USING ERRCODE = '40001';
      END IF;

      DELETE FROM brunn.record_keys
      WHERE user_id=target_stage.user_id
        AND record_id=candidate.asset_id
        AND record_kind='asset';
      GET DIAGNOSTICS affected = ROW_COUNT;
      IF affected <> 1 THEN
        RAISE EXCEPTION 'unpublished asset record key changed during reclamation'
          USING ERRCODE = '40001';
      END IF;
    ELSE
      UPDATE brunn.assets
      SET current_version=candidate.previous_version
      WHERE user_id=target_stage.user_id
        AND id=candidate.asset_id
        AND current_version=candidate.version;
      GET DIAGNOSTICS affected = ROW_COUNT;
      IF affected <> 1 THEN
        RAISE EXCEPTION 'asset head changed during stage reclamation'
          USING ERRCODE = '40001';
      END IF;

      DELETE FROM brunn.asset_versions
      WHERE user_id=target_stage.user_id
        AND asset_id=candidate.asset_id
        AND version=candidate.version;
      GET DIAGNOSTICS affected = ROW_COUNT;
      IF affected <> 1 THEN
        RAISE EXCEPTION 'unpublished asset version changed during reclamation'
          USING ERRCODE = '40001';
      END IF;
    END IF;

    PERFORM set_config('brunn.asset_internal_operation','',true);
    reclaimed_keys := array_append(reclaimed_keys,candidate.object_key);
  END LOOP;

  DELETE FROM brunn.staged_entries
  WHERE user_id=target_stage.user_id AND stage_id=p_stage_id;

  UPDATE brunn.asset_uploads
  SET status='expired',updated_at=clock_timestamp(),
      failure_code='stage_expired'
  WHERE user_id=target_stage.user_id
    AND stage_id=p_stage_id
    AND status='consumed';

  RETURN QUERY
  SELECT DISTINCT reclaimed.key
  FROM unnest(reclaimed_keys) AS reclaimed(key)
  WHERE reclaimed.key IS NOT NULL;
EXCEPTION WHEN OTHERS THEN
  PERFORM set_config('brunn.asset_internal_operation','',true);
  RAISE;
END;
$$;

REVOKE ALL ON FUNCTION brunn.expire_unpromoted_stage(uuid)
FROM PUBLIC,app_rw,app_ro;

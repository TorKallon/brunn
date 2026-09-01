-- A provider restore may move objects into a differently named bucket as well
-- as issue different version IDs. Remap both parts of the physical locator in
-- the same administrator-only transaction; all logical identity and bytes stay
-- immutable.

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
  source_bucket text;
  restored_bucket text;
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
      source_bucket :=
        current_setting('brunn.asset_recovery_source_bucket', true);
      restored_bucket :=
        current_setting('brunn.asset_recovery_target_bucket', true);
      IF OLD.object_version_id IS NULL
         OR NEW.object_version_id IS NULL
         OR btrim(NEW.object_version_id) IN ('', 'null')
         OR source_bucket IS NULL
         OR restored_bucket IS NULL
         OR btrim(source_bucket) = ''
         OR btrim(restored_bucket) = ''
         OR OLD.bucket <> source_bucket
         OR NEW.bucket <> restored_bucket THEN
        RAISE EXCEPTION
          'asset recovery requires exact source and target storage locators'
          USING ERRCODE = '55000';
      END IF;
      IF (old_shape - 'object_version_id' - 'bucket')
           IS DISTINCT FROM (new_shape - 'object_version_id' - 'bucket') THEN
        RAISE EXCEPTION
          'asset recovery may only remap bucket and object_version_id'
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
    RAISE EXCEPTION
      '% is immutable; create a new revision or lineage record',
      TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;

  SELECT role.rolsuper
      OR pg_has_role(
        current_user,
        pg_get_userbyid(database.datdba),
        'MEMBER'
      )
  INTO administrator
  FROM pg_roles AS role
  JOIN pg_database AS database
    ON database.datname = current_database()
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
    RAISE EXCEPTION
      '% is immutable outside an administrator deletion job',
      TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;

  IF TG_NARGS = 0 THEN
    RAISE EXCEPTION
      'no redaction columns are configured for %',
      TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;

  FOR argument_index IN 0..TG_NARGS - 1 LOOP
    IF NOT old_shape ? TG_ARGV[argument_index] THEN
      RAISE EXCEPTION
        'unknown redaction column %.%',
        TG_TABLE_NAME,
        TG_ARGV[argument_index]
        USING ERRCODE = '42703';
    END IF;
    old_shape := old_shape - TG_ARGV[argument_index];
    new_shape := new_shape - TG_ARGV[argument_index];
  END LOOP;

  IF old_shape IS DISTINCT FROM new_shape THEN
    RAISE EXCEPTION
      'deletion redaction attempted to mutate protected columns on %',
      TG_TABLE_NAME
      USING ERRCODE = '55000';
  END IF;

  RETURN NEW;
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
    RAISE EXCEPTION
      'asset locator recovery requires a database administrator'
      USING ERRCODE = '42501';
  END IF;
  IF p_mapping IS NULL OR jsonb_typeof(p_mapping) <> 'array' THEN
    RAISE EXCEPTION
      'asset locator recovery mapping must be a JSON array'
      USING ERRCODE = '22023';
  END IF;

  CREATE TEMP TABLE pg_temp.asset_storage_locator_recovery_map (
    object_key text NOT NULL,
    source_version_id text NOT NULL,
    restored_version_id text NOT NULL,
    source_bucket text NOT NULL,
    restored_bucket text NOT NULL,
    content_hash text NOT NULL,
    size_bytes bigint NOT NULL,
    PRIMARY KEY (source_bucket,object_key,source_version_id),
    UNIQUE (restored_bucket,object_key,restored_version_id),
    CHECK (length(btrim(object_key)) > 0),
    CHECK (length(btrim(source_bucket)) > 0),
    CHECK (length(btrim(restored_bucket)) > 0),
    CHECK (
      length(btrim(source_version_id)) > 0
      AND source_version_id <> 'null'
    ),
    CHECK (
      length(btrim(restored_version_id)) > 0
      AND restored_version_id <> 'null'
    ),
    CHECK (content_hash ~ '^[0-9a-f]{64}$'),
    CHECK (size_bytes >= 0)
  ) ON COMMIT DROP;

  INSERT INTO pg_temp.asset_storage_locator_recovery_map (
    object_key,
    source_version_id,
    restored_version_id,
    source_bucket,
    restored_bucket,
    content_hash,
    size_bytes
  )
  SELECT item.object_key,
         item.source_version_id,
         item.restored_version_id,
         item.source_bucket,
         item.restored_bucket,
         item.content_hash,
         item.size_bytes
  FROM jsonb_to_recordset(p_mapping) AS item(
    object_key text,
    source_version_id text,
    restored_version_id text,
    source_bucket text,
    restored_bucket text,
    content_hash text,
    size_bytes bigint
  );

  IF (
    SELECT count(DISTINCT (source_bucket,restored_bucket))
    FROM pg_temp.asset_storage_locator_recovery_map
  ) > 1 THEN
    RAISE EXCEPTION
      'asset locator recovery mapping crosses multiple bucket pairs'
      USING ERRCODE = '22023';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM (
      SELECT source_bucket AS bucket,
             object_key,
             source_version_id AS version_id
      FROM pg_temp.asset_storage_locator_recovery_map
      UNION ALL
      SELECT restored_bucket,
             object_key,
             restored_version_id
      FROM pg_temp.asset_storage_locator_recovery_map
    ) AS identifiers
    GROUP BY bucket,object_key,version_id
    HAVING count(*) > 1
  ) THEN
    RAISE EXCEPTION
      'asset locator recovery mapping contains ambiguous source or target IDs'
      USING ERRCODE = '22023';
  END IF;

  PERFORM 1
  FROM brunn.asset_versions AS version
  WHERE version.object_version_id IS NOT NULL
  ORDER BY version.user_id,version.asset_id,version.version
  FOR UPDATE;

  IF EXISTS (
    SELECT 1
    FROM brunn.asset_versions AS version
    LEFT JOIN pg_temp.asset_storage_locator_recovery_map AS mapping
      ON mapping.object_key=version.object_key
     AND (
       (
         version.bucket=mapping.source_bucket
         AND version.object_version_id=mapping.source_version_id
       )
       OR
       (
         version.bucket=mapping.restored_bucket
         AND version.object_version_id=mapping.restored_version_id
       )
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
           version.bucket AS source_bucket,
           mapping.restored_bucket,
           version.object_version_id AS source_version_id,
           mapping.restored_version_id
    FROM brunn.asset_versions AS version
    JOIN pg_temp.asset_storage_locator_recovery_map AS mapping
      ON mapping.source_bucket=version.bucket
     AND mapping.object_key=version.object_key
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
    PERFORM set_config(
      'brunn.asset_recovery_source_bucket',
      candidate.source_bucket,
      true
    );
    PERFORM set_config(
      'brunn.asset_recovery_target_bucket',
      candidate.restored_bucket,
      true
    );

    UPDATE brunn.asset_versions AS version
    SET bucket=candidate.restored_bucket,
        object_version_id=candidate.restored_version_id
    WHERE version.user_id=candidate.user_id
      AND version.asset_id=candidate.asset_id
      AND version.version=candidate.version
      AND version.bucket=candidate.source_bucket
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
    PERFORM set_config(
      'brunn.asset_recovery_source_bucket',
      '',
      true
    );
    PERFORM set_config(
      'brunn.asset_recovery_target_bucket',
      '',
      true
    );
  END LOOP;

  IF EXISTS (
    SELECT 1
    FROM brunn.asset_versions AS version
    LEFT JOIN pg_temp.asset_storage_locator_recovery_map AS mapping
      ON mapping.restored_bucket=version.bucket
     AND mapping.object_key=version.object_key
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
  PERFORM set_config('brunn.asset_recovery_source_bucket','',true);
  PERFORM set_config('brunn.asset_recovery_target_bucket','',true);
  RETURN updated_count;
EXCEPTION WHEN OTHERS THEN
  PERFORM set_config('brunn.asset_internal_operation','',true);
  PERFORM set_config('brunn.asset_recovery_source_bucket','',true);
  PERFORM set_config('brunn.asset_recovery_target_bucket','',true);
  RAISE;
END;
$$;

REVOKE ALL ON FUNCTION brunn.remap_asset_object_versions(jsonb)
FROM PUBLIC,app_rw,app_ro;

COMMENT ON FUNCTION brunn.remap_asset_object_versions(jsonb) IS
  'Administrator-only disaster recovery remap for complete immutable asset storage locators, preserving logical identity, hash, size, and lineage.';

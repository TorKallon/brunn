-- Rows written before provider-version pinning used a content-addressed object
-- key but did not retain the provider's opaque version ID. Allow one narrowly
-- scoped administrator operation to attach a verified exact version without
-- weakening immutable asset history.

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
         'legacy_locator_pin',
         OLD.user_id,
         OLD.asset_id,
         OLD.version,
         OLD.object_version_id,
         NEW.object_version_id
       ) THEN
      IF OLD.object_version_id IS NOT NULL
         OR NEW.object_version_id IS NULL
         OR btrim(NEW.object_version_id) IN ('', 'null')
         OR OLD.bucket IS DISTINCT FROM NEW.bucket THEN
        RAISE EXCEPTION
          'legacy asset pinning requires a missing source version and exact target version'
          USING ERRCODE = '55000';
      END IF;
      IF (old_shape - 'object_version_id')
           IS DISTINCT FROM (new_shape - 'object_version_id') THEN
        RAISE EXCEPTION
          'legacy asset pinning may only set object_version_id'
          USING ERRCODE = '55000';
      END IF;
      RETURN NEW;
    END IF;

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

CREATE FUNCTION brunn.pin_legacy_asset_object_versions(p_mapping jsonb)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  candidate record;
  operation_context text;
  affected bigint;
  updated_count bigint := 0;
BEGIN
  IF NOT brunn.database_administrator() THEN
    RAISE EXCEPTION
      'legacy asset locator pinning requires a database administrator'
      USING ERRCODE = '42501';
  END IF;
  IF p_mapping IS NULL OR jsonb_typeof(p_mapping) <> 'array' THEN
    RAISE EXCEPTION
      'legacy asset locator mapping must be a JSON array'
      USING ERRCODE = '22023';
  END IF;

  CREATE TEMP TABLE pg_temp.legacy_asset_locator_map (
    user_id uuid NOT NULL,
    asset_id uuid NOT NULL,
    asset_version integer NOT NULL,
    object_key text NOT NULL,
    object_version_id text NOT NULL,
    content_hash text NOT NULL,
    size_bytes bigint NOT NULL,
    PRIMARY KEY (user_id, asset_id, asset_version),
    CHECK (length(btrim(object_key)) > 0),
    CHECK (
      length(btrim(object_version_id)) > 0
      AND object_version_id <> 'null'
    ),
    CHECK (content_hash ~ '^[0-9a-f]{64}$'),
    CHECK (size_bytes >= 0)
  ) ON COMMIT DROP;

  INSERT INTO pg_temp.legacy_asset_locator_map (
    user_id,
    asset_id,
    asset_version,
    object_key,
    object_version_id,
    content_hash,
    size_bytes
  )
  SELECT item.user_id,
         item.asset_id,
         item.asset_version,
         item.object_key,
         item.object_version_id,
         item.content_hash,
         item.size_bytes
  FROM jsonb_to_recordset(p_mapping) AS item(
    user_id uuid,
    asset_id uuid,
    asset_version integer,
    object_key text,
    object_version_id text,
    content_hash text,
    size_bytes bigint
  );

  IF EXISTS (
    SELECT 1
    FROM brunn.asset_versions AS version
    JOIN pg_temp.legacy_asset_locator_map AS mapping
      ON mapping.user_id=version.user_id
     AND mapping.asset_id=version.asset_id
     AND mapping.asset_version=version.version
    WHERE version.object_version_id IS NOT NULL
       OR version.object_key<>mapping.object_key
       OR version.content_hash<>mapping.content_hash
       OR version.size_bytes<>mapping.size_bytes
  ) OR (
    SELECT count(*) FROM pg_temp.legacy_asset_locator_map
  ) <> (
    SELECT count(*)
    FROM brunn.asset_versions AS version
    JOIN pg_temp.legacy_asset_locator_map AS mapping
      ON mapping.user_id=version.user_id
     AND mapping.asset_id=version.asset_id
     AND mapping.asset_version=version.version
  ) THEN
    RAISE EXCEPTION
      'legacy asset locator mapping does not exactly match unpinned immutable rows'
      USING ERRCODE = '22023';
  END IF;

  FOR candidate IN
    SELECT mapping.*
    FROM pg_temp.legacy_asset_locator_map AS mapping
    ORDER BY mapping.user_id, mapping.asset_id, mapping.asset_version
  LOOP
    operation_context := 'legacy_locator_pin:' || encode(
      public.digest(
        jsonb_build_array(
          candidate.user_id::text,
          candidate.asset_id::text,
          candidate.asset_version,
          NULL,
          candidate.object_version_id
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
    SET object_version_id=candidate.object_version_id
    WHERE version.user_id=candidate.user_id
      AND version.asset_id=candidate.asset_id
      AND version.version=candidate.asset_version
      AND version.object_key=candidate.object_key
      AND version.content_hash=candidate.content_hash
      AND version.size_bytes=candidate.size_bytes
      AND version.object_version_id IS NULL;
    GET DIAGNOSTICS affected = ROW_COUNT;
    IF affected <> 1 THEN
      RAISE EXCEPTION
        'legacy asset locator row changed while it was being pinned'
        USING ERRCODE = '40001';
    END IF;
    updated_count := updated_count + affected;
  END LOOP;

  PERFORM set_config('brunn.asset_internal_operation', '', true);
  RETURN updated_count;
END;
$$;

REVOKE ALL ON FUNCTION brunn.pin_legacy_asset_object_versions(jsonb)
FROM PUBLIC,app_rw,app_ro;

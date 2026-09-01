-- Make stage reclamation fail closed against every foreign key in the live
-- schema. This replaces the hand-maintained reference list in migration 0036.

CREATE OR REPLACE FUNCTION brunn.foreign_key_reference_exists(
  p_parent regclass,
  p_key jsonb,
  p_ignored_children regclass[] DEFAULT ARRAY[]::regclass[]
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  reference_constraint record;
  referenced boolean;
BEGIN
  FOR reference_constraint IN
    SELECT constraint_row.conrelid::regclass AS child_relation,
           array_agg(
             parent_attribute.attname
             ORDER BY child_key.ordinality
           ) AS parent_columns,
           string_agg(
             format(
               '%I IS NOT DISTINCT FROM (($1 ->> %L)::%s)',
               child_attribute.attname,
               parent_attribute.attname,
               format_type(
                 parent_attribute.atttypid,
                 parent_attribute.atttypmod
               )
             ),
             ' AND '
             ORDER BY child_key.ordinality
           ) AS predicate
    FROM pg_constraint AS constraint_row
    CROSS JOIN LATERAL unnest(constraint_row.conkey)
      WITH ORDINALITY AS child_key(attnum, ordinality)
    JOIN LATERAL unnest(constraint_row.confkey)
      WITH ORDINALITY AS parent_key(attnum, ordinality)
      ON parent_key.ordinality=child_key.ordinality
    JOIN pg_attribute AS child_attribute
      ON child_attribute.attrelid=constraint_row.conrelid
     AND child_attribute.attnum=child_key.attnum
    JOIN pg_attribute AS parent_attribute
      ON parent_attribute.attrelid=constraint_row.confrelid
     AND parent_attribute.attnum=parent_key.attnum
    WHERE constraint_row.contype='f'
      AND constraint_row.confrelid=p_parent
      AND NOT constraint_row.conrelid=ANY(p_ignored_children)
    GROUP BY constraint_row.oid,constraint_row.conrelid
  LOOP
    IF EXISTS (
      SELECT 1
      FROM unnest(reference_constraint.parent_columns) AS parent_column(name)
      WHERE NOT p_key ? parent_column.name
    ) THEN
      RAISE EXCEPTION
        'reclamation key for % does not cover foreign key columns %',
        p_parent,
        reference_constraint.parent_columns;
    END IF;

    EXECUTE format(
      'SELECT EXISTS (SELECT 1 FROM %s WHERE %s)',
      reference_constraint.child_relation,
      reference_constraint.predicate
    )
    INTO referenced
    USING p_key;

    IF referenced THEN
      RETURN true;
    END IF;
  END LOOP;
  RETURN false;
END;
$$;

REVOKE ALL ON FUNCTION brunn.foreign_key_reference_exists(
  regclass,
  jsonb,
  regclass[]
) FROM PUBLIC,app_rw,app_ro;

CREATE OR REPLACE FUNCTION brunn.expire_unpromoted_stage(p_stage_id uuid)
RETURNS TABLE(reclaimed_object_key text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  target_stage record;
  candidate record;
  reclaimed_keys text[] := ARRAY[]::text[];
BEGIN
  -- Discover the user without taking a row lock. Every writer takes the
  -- per-user storage lock before locking a stage or asset row.
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
  FOR candidate IN
    SELECT DISTINCT version.asset_id,version.version,version.previous_version,
           version.object_key,asset.current_version
    FROM brunn.staged_entries AS entry
    JOIN brunn.asset_versions AS version
      ON version.user_id=entry.user_id
     AND version.asset_id=entry.asset_id
     AND version.version=entry.asset_version
    JOIN brunn.assets AS asset
      ON asset.user_id=version.user_id AND asset.id=version.asset_id
    WHERE entry.user_id=target_stage.user_id
      AND entry.stage_id=p_stage_id
      AND NOT EXISTS (
        SELECT 1
        FROM brunn.staged_entries AS other_stage
        WHERE other_stage.user_id=entry.user_id
          AND other_stage.stage_id<>entry.stage_id
          AND other_stage.asset_id=entry.asset_id
          AND other_stage.asset_version=entry.asset_version
      )
      AND NOT EXISTS (
        SELECT 1
        FROM brunn.asset_versions AS successor
        WHERE successor.user_id=entry.user_id
          AND successor.asset_id=entry.asset_id
          AND successor.previous_version=entry.asset_version
      )
      AND asset.current_version=entry.asset_version
  LOOP
    IF brunn.foreign_key_reference_exists(
      'brunn.asset_versions'::regclass,
      jsonb_build_object(
        'user_id',target_stage.user_id::text,
        'asset_id',candidate.asset_id::text,
        'version',candidate.version::text
      ),
      ARRAY[
        'brunn.assets'::regclass,
        'brunn.asset_versions'::regclass,
        'brunn.staged_entries'::regclass
      ]
    ) THEN
      CONTINUE candidate_loop;
    END IF;

    IF candidate.previous_version IS NULL
       AND (
         brunn.foreign_key_reference_exists(
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

    DELETE FROM brunn.staged_entries
    WHERE user_id=target_stage.user_id
      AND stage_id=p_stage_id
      AND asset_id=candidate.asset_id
      AND asset_version=candidate.version;

    reclaimed_keys := array_append(reclaimed_keys, candidate.object_key);
    -- These rows never entered a corpus revision, but the ordinary immutable
    -- and head-advance triggers cannot distinguish them from durable history.
    -- All live foreign-key references were checked above while the user
    -- storage lock is held, so suppress triggers only for this coordinated
    -- rollback of the unpublished asset revision.
    PERFORM set_config('session_replication_role','replica',true);
    IF candidate.previous_version IS NULL THEN
      DELETE FROM brunn.asset_versions
      WHERE user_id=target_stage.user_id
        AND asset_id=candidate.asset_id
        AND version=candidate.version;
      DELETE FROM brunn.assets
      WHERE user_id=target_stage.user_id AND id=candidate.asset_id;
      DELETE FROM brunn.record_keys
      WHERE user_id=target_stage.user_id
        AND record_id=candidate.asset_id
        AND record_kind='asset';
    ELSE
      UPDATE brunn.assets
      SET current_version=candidate.previous_version
      WHERE user_id=target_stage.user_id
        AND id=candidate.asset_id
        AND current_version=candidate.version;
      DELETE FROM brunn.asset_versions
      WHERE user_id=target_stage.user_id
        AND asset_id=candidate.asset_id
        AND version=candidate.version;
    END IF;
    PERFORM set_config('session_replication_role','origin',true);
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
  PERFORM set_config('session_replication_role','origin',true);
  RAISE;
END;
$$;

REVOKE ALL ON FUNCTION brunn.expire_unpromoted_stage(uuid)
FROM PUBLIC,app_rw,app_ro;

-- Reclaim unpromoted stage-only database state without weakening the
-- immutability rules for records that ever entered a corpus revision.

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
  stage_ref text := 'stage:' || p_stage_id::text;
BEGIN
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

  PERFORM pg_advisory_xact_lock(
    hashtextextended('storage:' || target_stage.user_id::text,0)
  );

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
        SELECT 1 FROM brunn.source_asset_links AS source_link
        WHERE source_link.user_id=entry.user_id
          AND source_link.asset_id=entry.asset_id
          AND source_link.asset_version=entry.asset_version
      )
      AND NOT EXISTS (
        SELECT 1 FROM brunn.evidence_items AS evidence
        WHERE evidence.user_id=entry.user_id
          AND evidence.asset_id=entry.asset_id
          AND evidence.asset_version=entry.asset_version
      )
      AND NOT EXISTS (
        SELECT 1 FROM brunn.artifact_asset_links AS artifact
        WHERE artifact.user_id=entry.user_id
          AND artifact.asset_id=entry.asset_id
          AND artifact.asset_version=entry.asset_version
      )
      AND NOT EXISTS (
        SELECT 1 FROM brunn.document_revisions AS document
        WHERE document.user_id=entry.user_id
          AND document.asset_id=entry.asset_id
          AND document.asset_version=entry.asset_version
      )
      AND NOT EXISTS (
        SELECT 1 FROM brunn.corpus_members AS member
        WHERE member.user_id=entry.user_id
          AND member.record_kind='asset'
          AND member.record_id=entry.asset_id
          AND member.record_version=entry.asset_version
      )
      AND NOT EXISTS (
        SELECT 1 FROM brunn.asset_versions AS successor
        WHERE successor.user_id=entry.user_id
          AND successor.asset_id=entry.asset_id
          AND successor.previous_version=entry.asset_version
      )
      AND asset.current_version=entry.asset_version
  LOOP
    reclaimed_keys := array_append(reclaimed_keys, candidate.object_key);
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

REVOKE ALL ON FUNCTION brunn.expire_unpromoted_stage(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn.expire_unpromoted_stage(uuid) FROM app_rw, app_ro;

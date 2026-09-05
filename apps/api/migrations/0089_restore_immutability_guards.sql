-- Restore guards removed by 0082's CASCADE, each with its own function.
-- Legacy deletion_jobs no longer exist. Evidence remains correction-by-new-row;
-- account deletion retains only its existing identity-bound audit redaction.
CREATE FUNCTION brunn.guard_asset_versions_immutable()
RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER
SET search_path = pg_catalog, brunn
AS $$
DECLARE
  old_shape jsonb := to_jsonb(OLD);
  new_shape jsonb := to_jsonb(NEW);
  source_bucket text;
  restored_bucket text;
BEGIN
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
  RAISE EXCEPTION 'asset_versions is immutable; create a new revision'
    USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER asset_versions_immutable
BEFORE UPDATE OR DELETE ON brunn.asset_versions
FOR EACH ROW EXECUTE FUNCTION brunn.guard_asset_versions_immutable();

CREATE FUNCTION brunn.guard_source_episodes_immutable()
RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER
AS $$
BEGIN
  RAISE EXCEPTION 'source_episodes is immutable; create a new revision'
    USING ERRCODE = '55000';
END;
$$;
CREATE TRIGGER source_episodes_immutable
BEFORE UPDATE OR DELETE ON brunn.source_episodes
FOR EACH ROW EXECUTE FUNCTION brunn.guard_source_episodes_immutable();

CREATE FUNCTION brunn.guard_evidence_items_immutable()
RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER
AS $$
BEGIN
  RAISE EXCEPTION 'evidence_items is immutable; create a new revision'
    USING ERRCODE = '55000';
END;
$$;
CREATE TRIGGER evidence_items_immutable
BEFORE UPDATE OR DELETE ON brunn.evidence_items
FOR EACH ROW EXECUTE FUNCTION brunn.guard_evidence_items_immutable();

CREATE FUNCTION brunn.guard_audit_events_deletion_redaction()
RETURNS trigger LANGUAGE plpgsql SECURITY INVOKER
SET search_path = pg_catalog, brunn
AS $$
DECLARE
  request_id uuid;
BEGIN
  IF TG_OP <> 'UPDATE' OR NOT brunn.database_administrator() THEN
    RAISE EXCEPTION 'audit_events is immutable outside an authorized deletion'
      USING ERRCODE = '55000';
  END IF;
  BEGIN
    request_id := nullif(current_setting('brunn.account_deletion_request_id', true), '')::uuid;
  EXCEPTION WHEN invalid_text_representation THEN
    request_id := NULL;
  END;
  IF request_id IS NULL OR NOT EXISTS (
    SELECT 1 FROM brunn.account_deletion_requests AS request
    WHERE request.id=request_id AND request.user_id=OLD.user_id
      AND request.status IN ('running', 'awaiting_backup_expiry')
  ) THEN
    RAISE EXCEPTION 'audit_events is immutable outside an authorized deletion'
      USING ERRCODE = '55000';
  END IF;
  IF (to_jsonb(OLD) - ARRAY['actor_ref','request_id','details','content_free'])
       IS DISTINCT FROM
     (to_jsonb(NEW) - ARRAY['actor_ref','request_id','details','content_free']) THEN
    RAISE EXCEPTION 'deletion redaction attempted to mutate protected audit columns'
      USING ERRCODE = '55000';
  END IF;
  RETURN NEW;
END;
$$;
CREATE TRIGGER audit_events_deletion_redaction
BEFORE UPDATE OR DELETE ON brunn.audit_events
FOR EACH ROW EXECUTE FUNCTION brunn.guard_audit_events_deletion_redaction();

REVOKE ALL ON FUNCTION brunn.guard_asset_versions_immutable(),
  brunn.guard_source_episodes_immutable(), brunn.guard_evidence_items_immutable(),
  brunn.guard_audit_events_deletion_redaction() FROM PUBLIC;

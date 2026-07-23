-- Immutable evidence/history rows may only lose content while an administrator
-- is executing a real deletion job. The ordinary application roles cannot
-- manufacture this context by setting the custom GUC themselves.
CREATE FUNCTION straylight.guard_deletion_redaction()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
AS $$
DECLARE
  deletion_job_setting text := current_setting('straylight.deletion_job_id', true);
  deletion_job_id uuid;
  row_user_id uuid;
  old_shape jsonb := to_jsonb(OLD);
  new_shape jsonb := to_jsonb(NEW);
  argument_index integer;
  administrator boolean := false;
BEGIN
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
       FROM straylight.deletion_jobs AS job
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

COMMENT ON FUNCTION straylight.guard_deletion_redaction() IS
  'Allows listed content columns to be redacted only by a DB administrator with a transaction-local GUC naming a propagating deletion job for the same user.';

-- Relation revisions retain their keys and lineage but no longer need to retain
-- the semantic predicate after an authorized deletion.
INSERT INTO straylight.relation_schema_revisions (
  predicate, version, display_name, qualifier_schema, allowed_state_machines,
  allow_extra_roles, inverse_label, reserved
) VALUES (
  'deleted.redacted', 1, 'Deleted relation', '{}'::jsonb, '{}'::text[],
  true, NULL, true
)
ON CONFLICT (predicate, version) DO NOTHING;

DO $$
DECLARE
  replacement record;
  function_arguments text;
BEGIN
  FOR replacement IN
    SELECT * FROM (VALUES
      ('asset_versions', 'asset_versions_immutable', ARRAY[
        'object_key', 'content_hash', 'size_bytes', 'media_type', 'etag',
        'encryption', 'metadata'
      ]::text[]),
      ('source_episodes', 'source_episodes_immutable', ARRAY[
        'source_ref', 'source_version', 'title', 'content_hash',
        'native_locator', 'metadata'
      ]::text[]),
      ('evidence_items', 'evidence_items_immutable', ARRAY[
        'locator', 'text_content', 'content_hash'
      ]::text[]),
      ('object_revisions', 'object_revisions_immutable', ARRAY[
        'label', 'properties'
      ]::text[]),
      ('event_series_revisions', 'event_series_revisions_immutable', ARRAY[
        'source_uid', 'scheduling_sequence'
      ]::text[]),
      ('event_occurrences', 'event_occurrences_immutable', ARRAY[
        'canonical_recurrence_id'
      ]::text[]),
      ('claims', 'claims_immutable', ARRAY[
        'predicate', 'value', 'authority', 'canonicality', 'confidence',
        'valid_temporal_id', 'observed_temporal_id'
      ]::text[]),
      ('relation_revisions', 'relation_revisions_immutable', ARRAY[
        'predicate', 'schema_version', 'qualifiers', 'valid_temporal_id'
      ]::text[]),
      ('identity_name_claims', 'identity_name_claims_immutable', ARRAY[
        'name_kind', 'name_value', 'locale'
      ]::text[]),
      ('external_identifiers', 'external_identifiers_immutable', ARRAY[
        'namespace', 'identifier_value', 'issuer'
      ]::text[]),
      ('checkpoints', 'checkpoints_immutable', ARRAY[
        'title', 'summary'
      ]::text[]),
      ('checkpoint_goals', 'checkpoint_goals_immutable', ARRAY[
        'goal_text'
      ]::text[]),
      ('checkpoint_gaps', 'checkpoint_gaps_immutable', ARRAY[
        'gap_kind', 'description'
      ]::text[]),
      ('checkpoint_gates', 'checkpoint_gates_immutable', ARRAY[
        'description'
      ]::text[]),
      ('checkpoint_actions', 'checkpoint_actions_immutable', ARRAY[
        'action_text', 'preconditions'
      ]::text[]),
      ('checkpoint_source_refs', 'checkpoint_source_refs_immutable', ARRAY[
        'locator'
      ]::text[]),
      ('document_revisions', 'document_revisions_immutable', ARRAY[
        'title', 'content_hash', 'native_locator', 'byte_size'
      ]::text[]),
      ('chunks', 'chunks_immutable', ARRAY[
        'heading_path', 'locator', 'content', 'content_hash', 'token_count',
        'search_vector'
      ]::text[]),
      ('retrieval_result_items', 'retrieval_result_items_immutable', ARRAY[
        'lane_scores', 'why_selected', 'evidence_refs', 'payload'
      ]::text[]),
      ('import_receipts', 'import_receipts_immutable', ARRAY[
        'stable_import_id', 'input_hash', 'inventory_hash', 'manifest_hash',
        'indexing_state', 'receipt'
      ]::text[]),
      ('import_entry_dispositions', 'import_entry_dispositions_immutable', ARRAY[
        'details'
      ]::text[]),
      ('tombstones', 'tombstones_immutable', ARRAY[
        'reason'
      ]::text[]),
      ('dream_region_members', 'dream_region_members_immutable', ARRAY[
        'expansion_reason'
      ]::text[]),
      ('dream_job_events', 'dream_job_events_immutable', ARRAY[
        'details'
      ]::text[]),
      ('dream_candidate_items', 'dream_candidate_items_immutable', ARRAY[
        'disposition', 'requires_review', 'source_refs', 'proposal'
      ]::text[]),
      ('dream_gate_results', 'dream_gate_results_immutable', ARRAY[
        'evidence_refs', 'details'
      ]::text[]),
      ('dream_evaluations', 'dream_evaluations_immutable', ARRAY[
        'result'
      ]::text[]),
      ('dream_reviews', 'dream_reviews_immutable', ARRAY[
        'reason'
      ]::text[]),
      ('dream_canary_observations', 'dream_canary_observations_immutable', ARRAY[
        'old_selection', 'new_selection', 'correction', 'source_invalidations'
      ]::text[]),
      ('dream_model_receipts', 'dream_model_receipts_immutable', ARRAY[
        'output', 'validation'
      ]::text[])
    ) AS configured(table_name, trigger_name, allowed_columns)
  LOOP
    SELECT string_agg(quote_literal(column_name), ',')
    INTO function_arguments
    FROM unnest(replacement.allowed_columns) AS column_name;

    EXECUTE format(
      'DROP TRIGGER %I ON straylight.%I',
      replacement.trigger_name,
      replacement.table_name
    );
    EXECUTE format(
      'CREATE TRIGGER %I BEFORE UPDATE OR DELETE ON straylight.%I '
      'FOR EACH ROW EXECUTE FUNCTION straylight.guard_deletion_redaction(%s)',
      replacement.trigger_name,
      replacement.table_name,
      function_arguments
    );
  END LOOP;
END;
$$;

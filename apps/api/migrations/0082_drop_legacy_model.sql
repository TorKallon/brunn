-- Migration 0082: drop the legacy memory-model and Phase-0 dreaming tables.
-- Every table below is empty in production and has no remaining code
-- reference after the legacy /v1/memory protocol and the Phase-0 shadow
-- dreaming pipeline were removed. Historical simple-side data
-- (staged_entries, stages, assets, asset_versions, source_episodes,
-- evidence_items, record_keys, scopes, policies) is intentionally retained.

DROP TABLE IF EXISTS straylight.account_deletion_targets CASCADE;
DROP TABLE IF EXISTS straylight.active_manifest_history CASCADE;
DROP TABLE IF EXISTS straylight.active_manifests CASCADE;
DROP TABLE IF EXISTS straylight.artifact_asset_links CASCADE;
DROP TABLE IF EXISTS straylight.asset_multipart_intents CASCADE;
DROP TABLE IF EXISTS straylight.asset_upload_parts CASCADE;
DROP TABLE IF EXISTS straylight.asset_uploads CASCADE;
DROP TABLE IF EXISTS straylight.checkpoint_actions CASCADE;
DROP TABLE IF EXISTS straylight.checkpoint_decisions CASCADE;
DROP TABLE IF EXISTS straylight.checkpoint_focus_links CASCADE;
DROP TABLE IF EXISTS straylight.checkpoint_gaps CASCADE;
DROP TABLE IF EXISTS straylight.checkpoint_gate_evidence CASCADE;
DROP TABLE IF EXISTS straylight.checkpoint_gates CASCADE;
DROP TABLE IF EXISTS straylight.checkpoint_goals CASCADE;
DROP TABLE IF EXISTS straylight.checkpoint_source_refs CASCADE;
DROP TABLE IF EXISTS straylight.checkpoint_state_refs CASCADE;
DROP TABLE IF EXISTS straylight.checkpoints CASCADE;
DROP TABLE IF EXISTS straylight.chunks CASCADE;
DROP TABLE IF EXISTS straylight.claim_about_targets CASCADE;
DROP TABLE IF EXISTS straylight.claim_evidence_links CASCADE;
DROP TABLE IF EXISTS straylight.claim_lineage CASCADE;
DROP TABLE IF EXISTS straylight.claims CASCADE;
DROP TABLE IF EXISTS straylight.continuation_tokens CASCADE;
DROP TABLE IF EXISTS straylight.corpus_members CASCADE;
DROP TABLE IF EXISTS straylight.corpus_revision_counters CASCADE;
DROP TABLE IF EXISTS straylight.corpus_revisions CASCADE;
DROP TABLE IF EXISTS straylight.deletion_job_events CASCADE;
DROP TABLE IF EXISTS straylight.deletion_jobs CASCADE;
DROP TABLE IF EXISTS straylight.derivative_cleanup_targets CASCADE;
DROP TABLE IF EXISTS straylight.document_revisions CASCADE;
DROP TABLE IF EXISTS straylight.documents CASCADE;
DROP TABLE IF EXISTS straylight.dream_canary_observations CASCADE;
DROP TABLE IF EXISTS straylight.dream_candidate_items CASCADE;
DROP TABLE IF EXISTS straylight.dream_candidate_revisions CASCADE;
DROP TABLE IF EXISTS straylight.dream_evaluations CASCADE;
DROP TABLE IF EXISTS straylight.dream_gate_results CASCADE;
DROP TABLE IF EXISTS straylight.dream_job_events CASCADE;
DROP TABLE IF EXISTS straylight.dream_jobs CASCADE;
DROP TABLE IF EXISTS straylight.dream_model_receipts CASCADE;
DROP TABLE IF EXISTS straylight.dream_promotion_receipts CASCADE;
DROP TABLE IF EXISTS straylight.dream_region_locks CASCADE;
DROP TABLE IF EXISTS straylight.dream_region_members CASCADE;
DROP TABLE IF EXISTS straylight.dream_regions CASCADE;
DROP TABLE IF EXISTS straylight.dream_reviews CASCADE;
DROP TABLE IF EXISTS straylight.dream_rollback_receipts CASCADE;
DROP TABLE IF EXISTS straylight.dream_scheduler_controls CASCADE;
DROP TABLE IF EXISTS straylight.embeddings CASCADE;
DROP TABLE IF EXISTS straylight.event_occurrence_revisions CASCADE;
DROP TABLE IF EXISTS straylight.event_occurrences CASCADE;
DROP TABLE IF EXISTS straylight.event_series_revisions CASCADE;
DROP TABLE IF EXISTS straylight.external_identifiers CASCADE;
DROP TABLE IF EXISTS straylight.field_policy_bindings CASCADE;
DROP TABLE IF EXISTS straylight.identity_name_claims CASCADE;
DROP TABLE IF EXISTS straylight.identity_review_receipts CASCADE;
DROP TABLE IF EXISTS straylight.import_entry_dispositions CASCADE;
DROP TABLE IF EXISTS straylight.import_receipts CASCADE;
DROP TABLE IF EXISTS straylight.materialization_cache CASCADE;
DROP TABLE IF EXISTS straylight.object_handles CASCADE;
DROP TABLE IF EXISTS straylight.object_revision_profiles CASCADE;
DROP TABLE IF EXISTS straylight.object_revisions CASCADE;
DROP TABLE IF EXISTS straylight.objects CASCADE;
DROP TABLE IF EXISTS straylight.policy_bindings CASCADE;
DROP TABLE IF EXISTS straylight.policy_rules CASCADE;
DROP TABLE IF EXISTS straylight.profile_schema_revisions CASCADE;
DROP TABLE IF EXISTS straylight.projection_receipts CASCADE;
DROP TABLE IF EXISTS straylight.record_access_events CASCADE;
DROP TABLE IF EXISTS straylight.recurrence_additions CASCADE;
DROP TABLE IF EXISTS straylight.recurrence_exclusions CASCADE;
DROP TABLE IF EXISTS straylight.recurrence_rules CASCADE;
DROP TABLE IF EXISTS straylight.recurrence_specs CASCADE;
DROP TABLE IF EXISTS straylight.redaction_transforms CASCADE;
DROP TABLE IF EXISTS straylight.relation_endpoints CASCADE;
DROP TABLE IF EXISTS straylight.relation_evidence_links CASCADE;
DROP TABLE IF EXISTS straylight.relation_revisions CASCADE;
DROP TABLE IF EXISTS straylight.relation_role_rules CASCADE;
DROP TABLE IF EXISTS straylight.relation_schema_revisions CASCADE;
DROP TABLE IF EXISTS straylight.relations CASCADE;
DROP TABLE IF EXISTS straylight.retrieval_operations CASCADE;
DROP TABLE IF EXISTS straylight.retrieval_result_items CASCADE;
DROP TABLE IF EXISTS straylight.session_root_refs CASCADE;
DROP TABLE IF EXISTS straylight.sessions CASCADE;
DROP TABLE IF EXISTS straylight.source_asset_links CASCADE;
DROP TABLE IF EXISTS straylight.state_assignments CASCADE;
DROP TABLE IF EXISTS straylight.state_heads CASCADE;
DROP TABLE IF EXISTS straylight.state_machines CASCADE;
DROP TABLE IF EXISTS straylight.state_transition_rules CASCADE;
DROP TABLE IF EXISTS straylight.state_values CASCADE;
DROP TABLE IF EXISTS straylight.temporal_specs CASCADE;
DROP TABLE IF EXISTS straylight.tombstones CASCADE;
DROP TABLE IF EXISTS straylight.write_operation_items CASCADE;
DROP TABLE IF EXISTS straylight.write_operations CASCADE;

DROP FUNCTION IF EXISTS straylight.expire_unpromoted_stage(uuid);

-- The deletion-redaction guards and legacy validators reference dropped
-- tables from triggers on retained tables; user-defaults seeding loses its
-- policy_rules insert; admin provisioning loses the manifest chain. The
-- unused p_empty_manifest_hash parameter is retained so callers do not
-- change shape.

DROP FUNCTION IF EXISTS straylight.guard_deletion_redaction() CASCADE;
DROP FUNCTION IF EXISTS straylight.guard_account_deletion_redaction() CASCADE;
DROP FUNCTION IF EXISTS straylight.allocate_corpus_revision_number() CASCADE;
DROP FUNCTION IF EXISTS straylight.ensure_profile_schema(uuid, text) CASCADE;
DROP FUNCTION IF EXISTS straylight.ensure_relation_schema(uuid, text) CASCADE;
DROP FUNCTION IF EXISTS straylight.read_manifest_stats(uuid) CASCADE;
DROP FUNCTION IF EXISTS straylight.read_manifest_sample(uuid, integer) CASCADE;
DROP FUNCTION IF EXISTS straylight_auth.admin_provision_user(text, text, text, text, text);

CREATE OR REPLACE FUNCTION straylight.seed_user_defaults()
RETURNS trigger
LANGUAGE plpgsql
AS $seed$
DECLARE
  default_policy_id uuid := gen_random_uuid();
BEGIN
  INSERT INTO straylight.scopes (id, user_id, scope_ref, name)
  VALUES (gen_random_uuid(), NEW.id, 'scope:root', 'Private root');

  INSERT INTO straylight.policies (
    id, user_id, policy_ref, name, current_version, is_default
  ) VALUES (
    default_policy_id, NEW.id, 'policy:private-default',
    'Private by default', 1, true
  );

  INSERT INTO straylight.policy_revisions (
    user_id, policy_id, version, previous_version, default_effect, rules
  ) VALUES (
    NEW.id,
    default_policy_id,
    1,
    NULL,
    'deny',
    jsonb_build_array(jsonb_build_object(
      'effect', 'allow',
      'principals', jsonb_build_array('principal:self'),
      'purposes', jsonb_build_array('*'),
      'actions', jsonb_build_array('read', 'write', 'derive', 'export'),
      'paths', jsonb_build_array('')
    ))
  );

  RETURN NEW;
END;
$seed$;

CREATE OR REPLACE FUNCTION straylight_auth.admin_provision_user(p_external_ref text, p_display_name text, p_credential_label text, p_token_hash text, p_empty_manifest_hash text)
 RETURNS TABLE(user_id uuid, credential_id uuid, scope_id uuid, policy_id uuid)
 LANGUAGE plpgsql
 SECURITY DEFINER
 SET search_path TO 'pg_catalog', 'straylight', 'straylight_auth'
 SET row_security TO 'off'
AS $function$
DECLARE
  owner_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'notification:publish', 'notification:manage',
    'secret:read', 'secret:write',
    'task.read', 'task.write', 'integration.manage',
    'message.read', 'message.write',
    'admin'
  ];
  created_user_id uuid;
  created_credential_id uuid;
  root_scope_id uuid;
  default_policy_id uuid;
BEGIN
  PERFORM straylight_auth.require_admin();
  IF p_external_ref IS NULL OR btrim(p_external_ref) = ''
     OR length(p_external_ref) > 200
     OR p_display_name IS NULL OR btrim(p_display_name) = ''
     OR length(p_display_name) > 200
     OR p_credential_label IS NULL OR btrim(p_credential_label) = ''
     OR length(p_credential_label) > 120 THEN
    RAISE EXCEPTION 'provisioning names are invalid' USING ERRCODE = '22023';
  END IF;
  IF EXISTS (
    SELECT 1 FROM straylight.users AS existing_user
    WHERE existing_user.external_ref = p_external_ref
  ) THEN
    RAISE EXCEPTION 'external_ref already exists' USING ERRCODE = '23505';
  END IF;
  INSERT INTO straylight.users (external_ref, display_name)
  VALUES (p_external_ref, p_display_name)
  RETURNING users.id INTO created_user_id;
  SELECT scope_row.id INTO root_scope_id
  FROM straylight.scopes AS scope_row
  WHERE scope_row.user_id = created_user_id
    AND scope_row.scope_ref = 'scope:root';
  SELECT policy_row.id INTO default_policy_id
  FROM straylight.policies AS policy_row
  WHERE policy_row.user_id = created_user_id
    AND policy_row.is_default;
  INSERT INTO straylight.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    created_user_id, p_credential_label, p_token_hash, owner_capabilities
  ) RETURNING api_credentials.id INTO created_credential_id;
  INSERT INTO straylight.credential_scope_grants (
    credential_id, user_id, scope_id
  ) VALUES (created_credential_id, created_user_id, root_scope_id);
  INSERT INTO straylight.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    straylight_auth.current_user_id(),
    straylight_auth.current_credential_id(),
    'admin.user.provision',
    jsonb_build_object('target_user_id', created_user_id),
    true
  );
  RETURN QUERY SELECT
    created_user_id, created_credential_id, root_scope_id,
    default_policy_id;
END;
$function$

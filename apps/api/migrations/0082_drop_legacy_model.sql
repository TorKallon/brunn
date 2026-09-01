-- Migration 0082: drop the legacy memory-model and Phase-0 dreaming tables.
-- Every table below is empty in production and has no remaining code
-- reference after the legacy /v1/memory protocol and the Phase-0 shadow
-- dreaming pipeline were removed. Historical simple-side data
-- (staged_entries, stages, assets, asset_versions, source_episodes,
-- evidence_items, record_keys, scopes, policies) is intentionally retained.

DROP TABLE IF EXISTS brunn.account_deletion_targets CASCADE;
DROP TABLE IF EXISTS brunn.active_manifest_history CASCADE;
DROP TABLE IF EXISTS brunn.active_manifests CASCADE;
DROP TABLE IF EXISTS brunn.artifact_asset_links CASCADE;
DROP TABLE IF EXISTS brunn.asset_multipart_intents CASCADE;
DROP TABLE IF EXISTS brunn.asset_upload_parts CASCADE;
DROP TABLE IF EXISTS brunn.asset_uploads CASCADE;
DROP TABLE IF EXISTS brunn.checkpoint_actions CASCADE;
DROP TABLE IF EXISTS brunn.checkpoint_decisions CASCADE;
DROP TABLE IF EXISTS brunn.checkpoint_focus_links CASCADE;
DROP TABLE IF EXISTS brunn.checkpoint_gaps CASCADE;
DROP TABLE IF EXISTS brunn.checkpoint_gate_evidence CASCADE;
DROP TABLE IF EXISTS brunn.checkpoint_gates CASCADE;
DROP TABLE IF EXISTS brunn.checkpoint_goals CASCADE;
DROP TABLE IF EXISTS brunn.checkpoint_source_refs CASCADE;
DROP TABLE IF EXISTS brunn.checkpoint_state_refs CASCADE;
DROP TABLE IF EXISTS brunn.checkpoints CASCADE;
DROP TABLE IF EXISTS brunn.chunks CASCADE;
DROP TABLE IF EXISTS brunn.claim_about_targets CASCADE;
DROP TABLE IF EXISTS brunn.claim_evidence_links CASCADE;
DROP TABLE IF EXISTS brunn.claim_lineage CASCADE;
DROP TABLE IF EXISTS brunn.claims CASCADE;
DROP TABLE IF EXISTS brunn.continuation_tokens CASCADE;
DROP TABLE IF EXISTS brunn.corpus_members CASCADE;
DROP TABLE IF EXISTS brunn.corpus_revision_counters CASCADE;
DROP TABLE IF EXISTS brunn.corpus_revisions CASCADE;
DROP TABLE IF EXISTS brunn.deletion_job_events CASCADE;
DROP TABLE IF EXISTS brunn.deletion_jobs CASCADE;
DROP TABLE IF EXISTS brunn.derivative_cleanup_targets CASCADE;
DROP TABLE IF EXISTS brunn.document_revisions CASCADE;
DROP TABLE IF EXISTS brunn.documents CASCADE;
DROP TABLE IF EXISTS brunn.dream_canary_observations CASCADE;
DROP TABLE IF EXISTS brunn.dream_candidate_items CASCADE;
DROP TABLE IF EXISTS brunn.dream_candidate_revisions CASCADE;
DROP TABLE IF EXISTS brunn.dream_evaluations CASCADE;
DROP TABLE IF EXISTS brunn.dream_gate_results CASCADE;
DROP TABLE IF EXISTS brunn.dream_job_events CASCADE;
DROP TABLE IF EXISTS brunn.dream_jobs CASCADE;
DROP TABLE IF EXISTS brunn.dream_model_receipts CASCADE;
DROP TABLE IF EXISTS brunn.dream_promotion_receipts CASCADE;
DROP TABLE IF EXISTS brunn.dream_region_locks CASCADE;
DROP TABLE IF EXISTS brunn.dream_region_members CASCADE;
DROP TABLE IF EXISTS brunn.dream_regions CASCADE;
DROP TABLE IF EXISTS brunn.dream_reviews CASCADE;
DROP TABLE IF EXISTS brunn.dream_rollback_receipts CASCADE;
DROP TABLE IF EXISTS brunn.dream_scheduler_controls CASCADE;
DROP TABLE IF EXISTS brunn.embeddings CASCADE;
DROP TABLE IF EXISTS brunn.event_occurrence_revisions CASCADE;
DROP TABLE IF EXISTS brunn.event_occurrences CASCADE;
DROP TABLE IF EXISTS brunn.event_series_revisions CASCADE;
DROP TABLE IF EXISTS brunn.external_identifiers CASCADE;
DROP TABLE IF EXISTS brunn.field_policy_bindings CASCADE;
DROP TABLE IF EXISTS brunn.identity_name_claims CASCADE;
DROP TABLE IF EXISTS brunn.identity_review_receipts CASCADE;
DROP TABLE IF EXISTS brunn.import_entry_dispositions CASCADE;
DROP TABLE IF EXISTS brunn.import_receipts CASCADE;
DROP TABLE IF EXISTS brunn.materialization_cache CASCADE;
DROP TABLE IF EXISTS brunn.object_handles CASCADE;
DROP TABLE IF EXISTS brunn.object_revision_profiles CASCADE;
DROP TABLE IF EXISTS brunn.object_revisions CASCADE;
DROP TABLE IF EXISTS brunn.objects CASCADE;
DROP TABLE IF EXISTS brunn.policy_bindings CASCADE;
DROP TABLE IF EXISTS brunn.policy_rules CASCADE;
DROP TABLE IF EXISTS brunn.profile_schema_revisions CASCADE;
DROP TABLE IF EXISTS brunn.projection_receipts CASCADE;
DROP TABLE IF EXISTS brunn.record_access_events CASCADE;
DROP TABLE IF EXISTS brunn.recurrence_additions CASCADE;
DROP TABLE IF EXISTS brunn.recurrence_exclusions CASCADE;
DROP TABLE IF EXISTS brunn.recurrence_rules CASCADE;
DROP TABLE IF EXISTS brunn.recurrence_specs CASCADE;
DROP TABLE IF EXISTS brunn.redaction_transforms CASCADE;
DROP TABLE IF EXISTS brunn.relation_endpoints CASCADE;
DROP TABLE IF EXISTS brunn.relation_evidence_links CASCADE;
DROP TABLE IF EXISTS brunn.relation_revisions CASCADE;
DROP TABLE IF EXISTS brunn.relation_role_rules CASCADE;
DROP TABLE IF EXISTS brunn.relation_schema_revisions CASCADE;
DROP TABLE IF EXISTS brunn.relations CASCADE;
DROP TABLE IF EXISTS brunn.retrieval_operations CASCADE;
DROP TABLE IF EXISTS brunn.retrieval_result_items CASCADE;
DROP TABLE IF EXISTS brunn.session_root_refs CASCADE;
DROP TABLE IF EXISTS brunn.sessions CASCADE;
DROP TABLE IF EXISTS brunn.source_asset_links CASCADE;
DROP TABLE IF EXISTS brunn.state_assignments CASCADE;
DROP TABLE IF EXISTS brunn.state_heads CASCADE;
DROP TABLE IF EXISTS brunn.state_machines CASCADE;
DROP TABLE IF EXISTS brunn.state_transition_rules CASCADE;
DROP TABLE IF EXISTS brunn.state_values CASCADE;
DROP TABLE IF EXISTS brunn.temporal_specs CASCADE;
DROP TABLE IF EXISTS brunn.tombstones CASCADE;
DROP TABLE IF EXISTS brunn.write_operation_items CASCADE;
DROP TABLE IF EXISTS brunn.write_operations CASCADE;

DROP FUNCTION IF EXISTS brunn.expire_unpromoted_stage(uuid);

-- The deletion-redaction guards and legacy validators reference dropped
-- tables from triggers on retained tables; user-defaults seeding loses its
-- policy_rules insert; admin provisioning loses the manifest chain. The
-- unused p_empty_manifest_hash parameter is retained so callers do not
-- change shape.

DROP FUNCTION IF EXISTS brunn.guard_deletion_redaction() CASCADE;
DROP FUNCTION IF EXISTS brunn.guard_account_deletion_redaction() CASCADE;
DROP FUNCTION IF EXISTS brunn.allocate_corpus_revision_number() CASCADE;
DROP FUNCTION IF EXISTS brunn.ensure_profile_schema(uuid, text) CASCADE;
DROP FUNCTION IF EXISTS brunn.ensure_relation_schema(uuid, text) CASCADE;
DROP FUNCTION IF EXISTS brunn.read_manifest_stats(uuid) CASCADE;
DROP FUNCTION IF EXISTS brunn.read_manifest_sample(uuid, integer) CASCADE;
DROP FUNCTION IF EXISTS brunn_auth.admin_provision_user(text, text, text, text, text);

CREATE OR REPLACE FUNCTION brunn_auth.seed_user_defaults()
RETURNS trigger
LANGUAGE plpgsql
AS $seed$
DECLARE
  default_policy_id uuid := gen_random_uuid();
BEGIN
  INSERT INTO brunn.scopes (id, user_id, scope_ref, name)
  VALUES (gen_random_uuid(), NEW.id, 'scope:root', 'Private root');

  INSERT INTO brunn.policies (
    id, user_id, policy_ref, name, current_version, is_default
  ) VALUES (
    default_policy_id, NEW.id, 'policy:private-default',
    'Private by default', 1, true
  );

  INSERT INTO brunn.policy_revisions (
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

CREATE OR REPLACE FUNCTION brunn_auth.admin_provision_user(p_external_ref text, p_display_name text, p_credential_label text, p_token_hash text, p_empty_manifest_hash text)
 RETURNS TABLE(user_id uuid, credential_id uuid, scope_id uuid, policy_id uuid)
 LANGUAGE plpgsql
 SECURITY DEFINER
 SET search_path TO 'pg_catalog', 'brunn', 'brunn_auth'
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
  PERFORM brunn_auth.require_admin();
  IF p_external_ref IS NULL OR btrim(p_external_ref) = ''
     OR length(p_external_ref) > 200
     OR p_display_name IS NULL OR btrim(p_display_name) = ''
     OR length(p_display_name) > 200
     OR p_credential_label IS NULL OR btrim(p_credential_label) = ''
     OR length(p_credential_label) > 120 THEN
    RAISE EXCEPTION 'provisioning names are invalid' USING ERRCODE = '22023';
  END IF;
  IF EXISTS (
    SELECT 1 FROM brunn.users AS existing_user
    WHERE existing_user.external_ref = p_external_ref
  ) THEN
    RAISE EXCEPTION 'external_ref already exists' USING ERRCODE = '23505';
  END IF;
  INSERT INTO brunn.users (external_ref, display_name)
  VALUES (p_external_ref, p_display_name)
  RETURNING users.id INTO created_user_id;
  SELECT scope_row.id INTO root_scope_id
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = created_user_id
    AND scope_row.scope_ref = 'scope:root';
  SELECT policy_row.id INTO default_policy_id
  FROM brunn.policies AS policy_row
  WHERE policy_row.user_id = created_user_id
    AND policy_row.is_default;
  INSERT INTO brunn.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    created_user_id, p_credential_label, p_token_hash, owner_capabilities
  ) RETURNING api_credentials.id INTO created_credential_id;
  INSERT INTO brunn.credential_scope_grants (
    credential_id, user_id, scope_id
  ) VALUES (created_credential_id, created_user_id, root_scope_id);
  INSERT INTO brunn.audit_events (
    user_id, credential_id, action, details, content_free
  ) VALUES (
    brunn_auth.current_user_id(),
    brunn_auth.current_credential_id(),
    'admin.user.provision',
    jsonb_build_object('target_user_id', created_user_id),
    true
  );
  RETURN QUERY SELECT
    created_user_id, created_credential_id, root_scope_id,
    default_policy_id;
END;
$function$

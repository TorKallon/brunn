INSERT INTO brunn.profile_schema_revisions (
  profile_ref, version, display_name, properties_schema, reserved
) VALUES
  ('core.person', 1, 'Person', '{}'::jsonb, true),
  ('core.organization', 1, 'Organization', '{}'::jsonb, true),
  ('core.group', 1, 'Group', '{}'::jsonb, true),
  ('core.place', 1, 'Place', '{}'::jsonb, true),
  ('core.event', 1, 'Event', '{}'::jsonb, true),
  ('core.arrangement', 1, 'Arrangement', '{}'::jsonb, true),
  ('core.resource', 1, 'Resource', '{}'::jsonb, true),
  ('core.work_item', 1, 'Work item', '{}'::jsonb, true),
  ('core.artifact', 1, 'Artifact', '{}'::jsonb, true),
  ('core.domain_object', 1, 'Domain object', '{}'::jsonb, true),
  ('core.event_series', 1, 'Event series', '{}'::jsonb, true),
  ('core.event_occurrence', 1, 'Event occurrence', '{}'::jsonb, true);

INSERT INTO brunn.state_machines (
  state_machine_ref, display_name, version, target_kinds, description, reserved
) VALUES
  ('state:event.schedule@v1', 'Event schedule', 1, ARRAY['object'], 'Schedule authority state.', true),
  ('state:event.execution@v1', 'Event execution', 1, ARRAY['object'], 'Actual event execution state.', true),
  ('state:participation.response@v1', 'Participation response', 1, ARRAY['relation'], 'Participant response or commitment.', true),
  ('state:participation.attendance@v1', 'Participation attendance', 1, ARRAY['relation'], 'Observed attendance outcome.', true),
  ('state:notification.delivery@v1', 'Notification delivery', 1, ARRAY['relation'], 'Notification transport and acknowledgement.', true),
  ('state:arrangement.booking@v1', 'Arrangement booking', 1, ARRAY['object'], 'Reservation or booking state.', true),
  ('state:arrangement.payment@v1', 'Arrangement payment', 1, ARRAY['object'], 'Independent payment state.', true),
  ('state:arrangement.allocation@v1', 'Arrangement allocation', 1, ARRAY['object'], 'Independent allocation state.', true),
  ('state:arrangement.use@v1', 'Arrangement use', 1, ARRAY['object'], 'Actual use state.', true),
  ('state:resource.availability@v1', 'Resource availability', 1, ARRAY['object'], 'Resource availability state.', true),
  ('state:work.execution@v1', 'Work execution', 1, ARRAY['object'], 'Work progress state.', true),
  ('state:gate.validation@v1', 'Gate validation', 1, ARRAY['object'], 'Evidence-backed validation state.', true);

INSERT INTO brunn.state_values (
  state_machine_ref, value, display_order, terminal
) VALUES
  ('state:event.schedule@v1', 'tentative', 0, false),
  ('state:event.schedule@v1', 'confirmed', 1, false),
  ('state:event.schedule@v1', 'postponed', 2, false),
  ('state:event.schedule@v1', 'cancelled', 3, true),
  ('state:event.execution@v1', 'not_started', 0, false),
  ('state:event.execution@v1', 'in_progress', 1, false),
  ('state:event.execution@v1', 'completed', 2, true),
  ('state:event.execution@v1', 'aborted', 3, true),
  ('state:participation.response@v1', 'needs_action', 0, false),
  ('state:participation.response@v1', 'tentative', 1, false),
  ('state:participation.response@v1', 'accepted', 2, false),
  ('state:participation.response@v1', 'declined', 3, true),
  ('state:participation.response@v1', 'delegated', 4, false),
  ('state:participation.attendance@v1', 'unknown', 0, false),
  ('state:participation.attendance@v1', 'attended', 1, true),
  ('state:participation.attendance@v1', 'absent', 2, true),
  ('state:participation.attendance@v1', 'partial', 3, true),
  ('state:notification.delivery@v1', 'not_sent', 0, false),
  ('state:notification.delivery@v1', 'sent', 1, false),
  ('state:notification.delivery@v1', 'delivered', 2, false),
  ('state:notification.delivery@v1', 'failed', 3, true),
  ('state:notification.delivery@v1', 'acknowledged', 4, true),
  ('state:arrangement.booking@v1', 'proposed', 0, false),
  ('state:arrangement.booking@v1', 'held', 1, false),
  ('state:arrangement.booking@v1', 'confirmed', 2, false),
  ('state:arrangement.booking@v1', 'cancelled', 3, true),
  ('state:arrangement.booking@v1', 'expired', 4, true),
  ('state:arrangement.payment@v1', 'not_due', 0, false),
  ('state:arrangement.payment@v1', 'unpaid', 1, false),
  ('state:arrangement.payment@v1', 'deposit_paid', 2, false),
  ('state:arrangement.payment@v1', 'paid', 3, true),
  ('state:arrangement.payment@v1', 'waived', 4, true),
  ('state:arrangement.payment@v1', 'refunded', 5, true),
  ('state:arrangement.allocation@v1', 'unallocated', 0, false),
  ('state:arrangement.allocation@v1', 'pending', 1, false),
  ('state:arrangement.allocation@v1', 'allocated', 2, false),
  ('state:arrangement.allocation@v1', 'waitlisted', 3, false),
  ('state:arrangement.allocation@v1', 'released', 4, true),
  ('state:arrangement.use@v1', 'not_started', 0, false),
  ('state:arrangement.use@v1', 'in_use', 1, false),
  ('state:arrangement.use@v1', 'used', 2, true),
  ('state:arrangement.use@v1', 'no_show', 3, true),
  ('state:resource.availability@v1', 'unknown', 0, false),
  ('state:resource.availability@v1', 'available', 1, false),
  ('state:resource.availability@v1', 'unavailable', 2, false),
  ('state:work.execution@v1', 'not_started', 0, false),
  ('state:work.execution@v1', 'in_progress', 1, false),
  ('state:work.execution@v1', 'blocked', 2, false),
  ('state:work.execution@v1', 'completed', 3, true),
  ('state:work.execution@v1', 'abandoned', 4, true),
  ('state:gate.validation@v1', 'not_started', 0, false),
  ('state:gate.validation@v1', 'attempted', 1, false),
  ('state:gate.validation@v1', 'passed', 2, true),
  ('state:gate.validation@v1', 'failed', 3, true),
  ('state:gate.validation@v1', 'blocked', 4, false),
  ('state:gate.validation@v1', 'not_applicable', 5, true);

INSERT INTO brunn.relation_schema_revisions (
  predicate, version, display_name, qualifier_schema,
  allowed_state_machines, allow_extra_roles, inverse_label, reserved
) VALUES
  (
    'core.possibly_same_as', 1, 'Possibly same as', '{}'::jsonb,
    '{}'::text[], false, 'possibly same as', true
  ),
  (
    'core.same_as', 1, 'Reviewed identity equivalence', '{}'::jsonb,
    '{}'::text[], false, 'same as', true
  ),
  (
    'core.participates_in', 1, 'Participates in', '{}'::jsonb,
    ARRAY[
      'state:participation.response@v1',
      'state:participation.attendance@v1',
      'state:notification.delivery@v1'
    ], false, 'has participant', true
  ),
  (
    'core.occurrence_of', 1, 'Occurrence of', '{}'::jsonb,
    ARRAY['state:event.schedule@v1', 'state:event.execution@v1'],
    false, 'has occurrence', true
  );

INSERT INTO brunn.relation_role_rules (
  predicate, schema_version, role, minimum_count, maximum_count, allowed_profiles
) VALUES
  ('core.possibly_same_as', 1, 'entity', 2, 2, ARRAY['core.person', 'core.organization', 'core.group']),
  ('core.same_as', 1, 'entity', 2, 2, ARRAY['core.person', 'core.organization', 'core.group']),
  ('core.participates_in', 1, 'participant', 1, NULL, ARRAY['core.person', 'core.organization', 'core.group']),
  ('core.participates_in', 1, 'event', 1, 1, ARRAY['core.event']),
  ('core.occurrence_of', 1, 'occurrence', 1, 1, ARRAY['core.event']),
  ('core.occurrence_of', 1, 'series', 1, 1, ARRAY['core.event']);

CREATE FUNCTION brunn.ensure_profile_schema(p_profile_ref text)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
AS $$
DECLARE
  validated_ref brunn.namespaced_identifier;
  resolved_version integer;
BEGIN
  IF NOT brunn_auth.context_is_valid()
     OR NOT brunn_auth.has_capability('save') THEN
    RAISE EXCEPTION 'authenticated save capability is required'
      USING ERRCODE = '42501';
  END IF;

  validated_ref := p_profile_ref::brunn.namespaced_identifier;

  SELECT max(schema_row.version) INTO resolved_version
  FROM brunn.profile_schema_revisions AS schema_row
  WHERE schema_row.profile_ref = validated_ref;

  IF resolved_version IS NOT NULL THEN
    RETURN resolved_version;
  END IF;

  IF p_profile_ref LIKE 'core.%' OR p_profile_ref LIKE 'state:%' THEN
    RAISE EXCEPTION 'unknown reserved profile schema: %', p_profile_ref
      USING ERRCODE = '22023';
  END IF;

  INSERT INTO brunn.profile_schema_revisions (
    profile_ref, version, display_name, properties_schema, reserved
  ) VALUES (
    validated_ref,
    1,
    p_profile_ref,
    '{"type":"object","additionalProperties":true}'::jsonb,
    false
  ) ON CONFLICT (profile_ref, version) DO NOTHING;

  SELECT max(schema_row.version) INTO STRICT resolved_version
  FROM brunn.profile_schema_revisions AS schema_row
  WHERE schema_row.profile_ref = validated_ref;

  RETURN resolved_version;
END;
$$;

CREATE FUNCTION brunn.ensure_relation_schema(p_predicate text)
RETURNS integer
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
AS $$
DECLARE
  validated_predicate brunn.namespaced_identifier;
  resolved_version integer;
BEGIN
  IF NOT brunn_auth.context_is_valid()
     OR NOT brunn_auth.has_capability('save') THEN
    RAISE EXCEPTION 'authenticated save capability is required'
      USING ERRCODE = '42501';
  END IF;

  validated_predicate := p_predicate::brunn.namespaced_identifier;

  SELECT max(schema_row.version) INTO resolved_version
  FROM brunn.relation_schema_revisions AS schema_row
  WHERE schema_row.predicate = validated_predicate;

  IF resolved_version IS NOT NULL THEN
    RETURN resolved_version;
  END IF;

  IF p_predicate LIKE 'core.%' OR p_predicate LIKE 'state:%' THEN
    RAISE EXCEPTION 'unknown reserved relation schema: %', p_predicate
      USING ERRCODE = '22023';
  END IF;

  INSERT INTO brunn.relation_schema_revisions (
    predicate, version, display_name, qualifier_schema,
    allowed_state_machines, allow_extra_roles, inverse_label, reserved
  ) VALUES (
    validated_predicate,
    1,
    p_predicate,
    '{"type":"object","additionalProperties":true}'::jsonb,
    '{}'::text[],
    true,
    NULL,
    false
  ) ON CONFLICT (predicate, version) DO NOTHING;

  SELECT max(schema_row.version) INTO STRICT resolved_version
  FROM brunn.relation_schema_revisions AS schema_row
  WHERE schema_row.predicate = validated_predicate;

  RETURN resolved_version;
END;
$$;

CREATE FUNCTION brunn_auth.require_save_control(p_user_id uuid)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
BEGIN
  IF NOT brunn_auth.context_is_valid()
     OR brunn_auth.current_user_id() IS DISTINCT FROM p_user_id
     OR NOT brunn_auth.has_capability('save') THEN
    RAISE EXCEPTION 'authenticated same-user save capability is required'
      USING ERRCODE = '42501';
  END IF;
END;
$$;

CREATE FUNCTION brunn_auth.create_scope(
  p_user_id uuid,
  p_scope_ref text,
  p_name text
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  created_scope_id uuid;
BEGIN
  PERFORM brunn_auth.require_save_control(p_user_id);

  INSERT INTO brunn.scopes (user_id, scope_ref, name)
  VALUES (p_user_id, p_scope_ref, p_name)
  ON CONFLICT (user_id, scope_ref) DO UPDATE
    SET name = EXCLUDED.name
  RETURNING id INTO created_scope_id;

  INSERT INTO brunn.credential_scope_grants (
    credential_id, user_id, scope_id
  ) VALUES (
    brunn_auth.current_credential_id(), p_user_id, created_scope_id
  ) ON CONFLICT DO NOTHING;

  INSERT INTO brunn.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    created_scope_id,
    brunn_auth.current_credential_id(),
    'auth.scope.create',
    jsonb_build_object('scope_id', created_scope_id, 'scope_ref', p_scope_ref),
    true
  );

  RETURN created_scope_id;
END;
$$;

CREATE FUNCTION brunn_auth.issue_credential(
  p_user_id uuid,
  p_label text,
  p_token_hash text,
  p_capabilities text[],
  p_scope_refs text[]
)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  allowed_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream'
  ];
  created_credential_id uuid;
  matched_scope_count integer;
BEGIN
  PERFORM brunn_auth.require_save_control(p_user_id);

  IF p_capabilities IS NULL
     OR cardinality(p_capabilities) = 0
     OR array_position(p_capabilities, NULL) IS NOT NULL
     OR NOT p_capabilities <@ allowed_capabilities
     OR cardinality(p_capabilities) <> (
       SELECT count(DISTINCT capability)
       FROM unnest(p_capabilities) AS capability
     ) THEN
    RAISE EXCEPTION 'capabilities contain an unknown, null, or duplicate value'
      USING ERRCODE = '22023';
  END IF;

  IF p_scope_refs IS NULL
     OR cardinality(p_scope_refs) = 0
     OR array_position(p_scope_refs, NULL) IS NOT NULL
     OR cardinality(p_scope_refs) <> (
       SELECT count(DISTINCT scope_ref)
       FROM unnest(p_scope_refs) AS scope_ref
     ) THEN
    RAISE EXCEPTION 'scope_refs must be a nonempty set'
      USING ERRCODE = '22023';
  END IF;

  SELECT count(*) INTO matched_scope_count
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);

  IF matched_scope_count <> cardinality(p_scope_refs) THEN
    RAISE EXCEPTION 'one or more scope_refs do not belong to the user'
      USING ERRCODE = '22023';
  END IF;

  INSERT INTO brunn.api_credentials (
    user_id, label, token_hash, capabilities
  ) VALUES (
    p_user_id, p_label, p_token_hash, p_capabilities
  ) RETURNING id INTO created_credential_id;

  INSERT INTO brunn.credential_scope_grants (
    credential_id, user_id, scope_id
  )
  SELECT created_credential_id, p_user_id, scope_row.id
  FROM brunn.scopes AS scope_row
  WHERE scope_row.user_id = p_user_id
    AND scope_row.scope_ref::text = ANY(p_scope_refs);

  INSERT INTO brunn.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    NULL,
    brunn_auth.current_credential_id(),
    'auth.credential.issue',
    jsonb_build_object(
      'credential_id', created_credential_id,
      'capabilities', p_capabilities,
      'scope_refs', p_scope_refs
    ),
    true
  );

  RETURN created_credential_id;
END;
$$;

CREATE FUNCTION brunn_auth.revoke_credential(
  p_user_id uuid,
  p_credential_id uuid
)
RETURNS timestamptz
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  revoked_at timestamptz;
BEGIN
  PERFORM brunn_auth.require_save_control(p_user_id);

  UPDATE brunn.api_credentials AS credential
  SET disabled_at = coalesce(credential.disabled_at, clock_timestamp())
  WHERE credential.user_id = p_user_id
    AND credential.id = p_credential_id
  RETURNING credential.disabled_at INTO revoked_at;

  IF revoked_at IS NULL THEN
    RAISE EXCEPTION 'credential not found for user' USING ERRCODE = 'P0002';
  END IF;

  INSERT INTO brunn.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id,
    NULL,
    brunn_auth.current_credential_id(),
    'auth.credential.revoke',
    jsonb_build_object('credential_id', p_credential_id, 'revoked_at', revoked_at),
    true
  );

  RETURN revoked_at;
END;
$$;

CREATE FUNCTION brunn_auth.list_credentials(p_user_id uuid)
RETURNS TABLE (
  id uuid,
  label text,
  capabilities text[],
  scope_refs text[],
  created_at timestamptz,
  disabled_at timestamptz
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
BEGIN
  IF NOT brunn_auth.context_is_valid()
     OR brunn_auth.current_user_id() IS DISTINCT FROM p_user_id
     OR NOT brunn_auth.has_any_capability(ARRAY['status', 'read']) THEN
    RAISE EXCEPTION 'authenticated same-user status or read capability is required'
      USING ERRCODE = '42501';
  END IF;

  RETURN QUERY
  SELECT
    credential.id,
    credential.label::text,
    credential.capabilities,
    coalesce(
      array_agg(scope_row.scope_ref::text ORDER BY scope_row.scope_ref)
        FILTER (WHERE scope_row.id IS NOT NULL),
      '{}'::text[]
    ),
    credential.created_at,
    credential.disabled_at
  FROM brunn.api_credentials AS credential
  LEFT JOIN brunn.credential_scope_grants AS scope_grant
    ON scope_grant.user_id = credential.user_id
   AND scope_grant.credential_id = credential.id
  LEFT JOIN brunn.scopes AS scope_row
    ON scope_row.user_id = scope_grant.user_id
   AND scope_row.id = scope_grant.scope_id
  WHERE credential.user_id = p_user_id
  GROUP BY
    credential.id,
    credential.label,
    credential.capabilities,
    credential.created_at,
    credential.disabled_at
  ORDER BY credential.created_at, credential.id;
END;
$$;

CREATE FUNCTION brunn_auth.can_access_user(p_user_id uuid)
RETURNS boolean
LANGUAGE sql
STABLE
AS $$
  SELECT brunn_auth.context_is_valid()
    AND p_user_id = brunn_auth.current_user_id()
$$;

CREATE FUNCTION brunn_auth.can_access_optional_scope(
  p_user_id uuid,
  p_scope_id uuid
)
RETURNS boolean
LANGUAGE sql
STABLE
AS $$
  SELECT brunn_auth.can_access_user(p_user_id)
    AND (
      p_scope_id IS NULL
      OR brunn_auth.can_access_scope(p_user_id, p_scope_id)
    )
$$;

CREATE FUNCTION brunn_auth.can_access_record(
  p_user_id uuid,
  p_record_id uuid
)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
  SELECT EXISTS (
    SELECT 1
    FROM brunn.record_keys AS record_key
    WHERE record_key.user_id = p_user_id
      AND record_key.record_id = p_record_id
      AND brunn_auth.can_access_scope(record_key.user_id, record_key.scope_id)
  )
$$;

-- RLS is enabled and forced for every user-bearing table. Tables without an
-- explicit policy below therefore fail closed.
DO $$
DECLARE
  table_row record;
BEGIN
  FOR table_row IN
    SELECT table_name
    FROM information_schema.columns
    WHERE table_schema = 'brunn' AND column_name = 'user_id'
    GROUP BY table_name
  LOOP
    EXECUTE format('ALTER TABLE brunn.%I ENABLE ROW LEVEL SECURITY', table_row.table_name);
    EXECUTE format('ALTER TABLE brunn.%I FORCE ROW LEVEL SECURITY', table_row.table_name);
  END LOOP;
END;
$$;

-- users uses id rather than user_id, so it is not covered by the catalog loop.
ALTER TABLE brunn.users ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.users FORCE ROW LEVEL SECURITY;

CREATE POLICY users_self_select ON brunn.users
  FOR SELECT TO app_rw, app_ro
  USING (brunn_auth.can_access_user(id));

CREATE POLICY scopes_granted_select ON brunn.scopes
  FOR SELECT TO app_rw, app_ro
  USING (brunn_auth.can_access_scope(user_id, id));

CREATE POLICY credentials_self_select ON brunn.api_credentials
  FOR SELECT TO app_rw, app_ro
  USING (brunn_auth.can_access_user(user_id));

CREATE POLICY policies_self_select ON brunn.policies
  FOR SELECT TO app_rw, app_ro
  USING (brunn_auth.can_access_user(user_id));

CREATE POLICY policies_self_write ON brunn.policies
  FOR ALL TO app_rw
  USING (
    brunn_auth.can_access_user(user_id)
    AND brunn_auth.has_any_capability(ARRAY['save', 'correct'])
  )
  WITH CHECK (
    brunn_auth.can_access_user(user_id)
    AND brunn_auth.has_any_capability(ARRAY['save', 'correct'])
  );

DO $$
DECLARE
  table_name text;
BEGIN
  FOREACH table_name IN ARRAY ARRAY['policy_revisions', 'policy_rules']
  LOOP
    EXECUTE format(
      'CREATE POLICY user_select ON brunn.%I FOR SELECT TO app_rw, app_ro '
      'USING (brunn_auth.can_access_user(user_id))',
      table_name
    );
    EXECUTE format(
      'CREATE POLICY user_write ON brunn.%I FOR ALL TO app_rw '
      'USING (brunn_auth.can_access_user(user_id) AND '
      'brunn_auth.has_any_capability(ARRAY[''save'', ''correct''])) '
      'WITH CHECK (brunn_auth.can_access_user(user_id) AND '
      'brunn_auth.has_any_capability(ARRAY[''save'', ''correct'']))',
      table_name
    );
  END LOOP;
END;
$$;

-- Directly scoped tables use one common isolation predicate. Mutation
-- capabilities remain operation-specific by table family.
DO $$
DECLARE
  table_row record;
  mutation_capabilities text[];
BEGIN
  FOR table_row IN
    SELECT table_name
    FROM information_schema.columns
    WHERE table_schema = 'brunn'
      AND column_name = 'scope_id'
      AND table_name NOT IN ('scopes', 'audit_events')
    GROUP BY table_name
  LOOP
    mutation_capabilities := CASE
      WHEN table_row.table_name IN (
        'sessions', 'retrieval_operations', 'continuation_tokens',
        'materialization_cache'
      ) THEN ARRAY['open', 'query', 'read', 'compute', 'verify', 'status']
      WHEN table_row.table_name IN (
        'checkpoints', 'checkpoint_focus_links', 'checkpoint_goals',
        'checkpoint_state_refs', 'checkpoint_gaps', 'checkpoint_decisions',
        'checkpoint_gates', 'checkpoint_gate_evidence', 'checkpoint_actions',
        'checkpoint_source_refs'
      ) THEN ARRAY['checkpoint', 'save']
      WHEN table_row.table_name IN (
        'stages', 'staged_entries', 'import_receipts',
        'import_entry_dispositions'
      ) THEN ARRAY['stage', 'save']
      WHEN table_row.table_name IN (
        'tombstones', 'deletion_jobs', 'derivative_cleanup_targets',
        'deletion_job_events'
      ) THEN ARRAY['delete']
      WHEN table_row.table_name IN ('active_manifests', 'active_manifest_history')
        THEN ARRAY['save', 'checkpoint', 'correct', 'delete', 'stage', 'dream']
      WHEN table_row.table_name LIKE 'dream_%'
        THEN ARRAY['dream']
      WHEN table_row.table_name = 'assets'
        THEN ARRAY['save', 'correct', 'stage', 'dream']
      WHEN table_row.table_name = 'projection_receipts'
        THEN ARRAY['open', 'query', 'read', 'compute', 'verify', 'save', 'dream']
      WHEN table_row.table_name IN (
        'corpus_revisions', 'corpus_members', 'record_keys',
        'background_jobs', 'write_operations'
      ) THEN ARRAY['checkpoint', 'save', 'stage', 'correct', 'delete', 'dream']
      ELSE ARRAY['save', 'correct', 'dream']
    END;

    EXECUTE format(
      'CREATE POLICY scope_select ON brunn.%I FOR SELECT TO app_rw, app_ro '
      'USING (brunn_auth.can_access_scope(user_id, scope_id))',
      table_row.table_name
    );
    EXECUTE format(
      'CREATE POLICY scope_write ON brunn.%I FOR ALL TO app_rw '
      'USING (brunn_auth.can_access_scope(user_id, scope_id) AND '
      'brunn_auth.has_any_capability(%L::text[])) '
      'WITH CHECK (brunn_auth.can_access_scope(user_id, scope_id) AND '
      'brunn_auth.has_any_capability(%L::text[]))',
      table_row.table_name,
      mutation_capabilities::text,
      mutation_capabilities::text
    );
  END LOOP;
END;
$$;

CREATE POLICY audit_events_select ON brunn.audit_events
  FOR SELECT TO app_rw, app_ro
  USING (brunn_auth.can_access_optional_scope(user_id, scope_id));

CREATE POLICY audit_events_insert ON brunn.audit_events
  FOR INSERT TO app_rw
  WITH CHECK (
    brunn_auth.can_access_optional_scope(user_id, scope_id)
    AND brunn_auth.has_any_capability(ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream'
    ])
  );

-- Child tables inherit scope from a checked parent. Keeping scope out of these
-- rows avoids denormalized scope values that could drift from the parent.
DO $$
DECLARE
  child record;
BEGIN
  FOR child IN
    SELECT * FROM (VALUES
      ('asset_versions', 'brunn_auth.can_access_record(user_id, asset_id)', ARRAY['save', 'correct', 'stage', 'dream']::text[]),
      ('source_asset_links', 'brunn_auth.can_access_record(user_id, source_episode_id)', ARRAY['save', 'stage']::text[]),
      ('object_revisions', 'brunn_auth.can_access_record(user_id, object_id)', ARRAY['save', 'correct', 'dream']::text[]),
      ('object_revision_profiles', 'brunn_auth.can_access_record(user_id, object_id)', ARRAY['save', 'correct', 'dream']::text[]),
      ('artifact_asset_links', 'brunn_auth.can_access_record(user_id, object_id)', ARRAY['save', 'correct', 'dream']::text[]),
      ('recurrence_rules', 'brunn_auth.can_access_record(user_id, recurrence_spec_id)', ARRAY['save', 'correct']::text[]),
      ('recurrence_exclusions', 'brunn_auth.can_access_record(user_id, recurrence_spec_id)', ARRAY['save', 'correct']::text[]),
      ('event_series_revisions', 'brunn_auth.can_access_record(user_id, object_id)', ARRAY['save', 'correct']::text[]),
      ('event_occurrence_revisions', 'brunn_auth.can_access_record(user_id, occurrence_object_id)', ARRAY['save', 'correct']::text[]),
      ('recurrence_additions', 'brunn_auth.can_access_record(user_id, recurrence_spec_id)', ARRAY['save', 'correct']::text[]),
      ('claim_about_targets', 'brunn_auth.can_access_record(user_id, claim_id)', ARRAY['save', 'correct', 'dream']::text[]),
      ('claim_evidence_links', 'brunn_auth.can_access_record(user_id, claim_id)', ARRAY['save', 'correct', 'dream']::text[]),
      ('claim_lineage', 'brunn_auth.can_access_record(user_id, claim_id)', ARRAY['save', 'correct', 'dream']::text[]),
      ('relation_revisions', 'brunn_auth.can_access_record(user_id, relation_id)', ARRAY['save', 'correct', 'dream']::text[]),
      ('relation_endpoints', 'brunn_auth.can_access_record(user_id, relation_id)', ARRAY['save', 'correct', 'dream']::text[]),
      ('relation_evidence_links', 'brunn_auth.can_access_record(user_id, relation_id)', ARRAY['save', 'correct', 'dream']::text[]),
      ('identity_name_claims', 'brunn_auth.can_access_record(user_id, claim_id)', ARRAY['save', 'correct']::text[]),
      ('checkpoint_focus_links', 'brunn_auth.can_access_record(user_id, checkpoint_id)', ARRAY['checkpoint', 'save']::text[]),
      ('checkpoint_goals', 'brunn_auth.can_access_record(user_id, checkpoint_id)', ARRAY['checkpoint', 'save']::text[]),
      ('checkpoint_state_refs', 'brunn_auth.can_access_record(user_id, checkpoint_id)', ARRAY['checkpoint', 'save']::text[]),
      ('checkpoint_gaps', 'brunn_auth.can_access_record(user_id, checkpoint_id)', ARRAY['checkpoint', 'save']::text[]),
      ('checkpoint_decisions', 'brunn_auth.can_access_record(user_id, checkpoint_id)', ARRAY['checkpoint', 'save']::text[]),
      ('checkpoint_gates', 'brunn_auth.can_access_record(user_id, checkpoint_id)', ARRAY['checkpoint', 'save']::text[]),
      ('checkpoint_gate_evidence', 'brunn_auth.can_access_record(user_id, checkpoint_id)', ARRAY['checkpoint', 'save']::text[]),
      ('checkpoint_actions', 'brunn_auth.can_access_record(user_id, checkpoint_id)', ARRAY['checkpoint', 'save']::text[]),
      ('checkpoint_source_refs', 'brunn_auth.can_access_record(user_id, checkpoint_id)', ARRAY['checkpoint', 'save']::text[]),
      ('document_revisions', 'brunn_auth.can_access_record(user_id, document_id)', ARRAY['save', 'correct', 'dream']::text[])
    ) AS mapping(table_name, access_expression, capabilities)
  LOOP
    EXECUTE format(
      'CREATE POLICY parent_select ON brunn.%I FOR SELECT TO app_rw, app_ro USING (%s)',
      child.table_name,
      child.access_expression
    );
    EXECUTE format(
      'CREATE POLICY parent_write ON brunn.%I FOR ALL TO app_rw '
      'USING ((%s) AND brunn_auth.has_any_capability(%L::text[])) '
      'WITH CHECK ((%s) AND brunn_auth.has_any_capability(%L::text[]))',
      child.table_name,
      child.access_expression,
      child.capabilities::text,
      child.access_expression,
      child.capabilities::text
    );
  END LOOP;
END;
$$;

-- Remaining child tables have operational parents rather than record keys.
DO $$
DECLARE
  child record;
BEGIN
  FOR child IN
    SELECT * FROM (VALUES
      ('session_root_refs', 'sessions', 'session_id', ARRAY['open']::text[]),
      ('retrieval_result_items', 'retrieval_operations', 'operation_id', ARRAY['open', 'query', 'read', 'compute', 'verify']::text[]),
      ('write_operation_items', 'write_operations', 'operation_id', ARRAY['checkpoint', 'save', 'stage', 'correct', 'delete']::text[]),
      ('import_entry_dispositions', 'import_receipts', 'import_receipt_id', ARRAY['stage', 'save']::text[]),
      ('dream_region_members', 'dream_regions', 'region_id', ARRAY['dream']::text[]),
      ('dream_candidate_items', 'dream_candidate_revisions', 'candidate_revision_id', ARRAY['dream']::text[])
    ) AS mapping(table_name, parent_table, parent_column, capabilities)
  LOOP
    EXECUTE format(
      'CREATE POLICY parent_select ON brunn.%I FOR SELECT TO app_rw, app_ro '
      'USING (EXISTS (SELECT 1 FROM brunn.%I AS parent '
      'WHERE parent.user_id = %I.user_id AND parent.id = %I.%I '
      'AND brunn_auth.can_access_scope(parent.user_id, parent.scope_id)))',
      child.table_name,
      child.parent_table,
      child.table_name,
      child.table_name,
      child.parent_column
    );
    EXECUTE format(
      'CREATE POLICY parent_write ON brunn.%I FOR ALL TO app_rw '
      'USING (EXISTS (SELECT 1 FROM brunn.%I AS parent '
      'WHERE parent.user_id = %I.user_id AND parent.id = %I.%I '
      'AND brunn_auth.can_access_scope(parent.user_id, parent.scope_id)) '
      'AND brunn_auth.has_any_capability(%L::text[])) '
      'WITH CHECK (EXISTS (SELECT 1 FROM brunn.%I AS parent '
      'WHERE parent.user_id = %I.user_id AND parent.id = %I.%I '
      'AND brunn_auth.can_access_scope(parent.user_id, parent.scope_id)) '
      'AND brunn_auth.has_any_capability(%L::text[]))',
      child.table_name,
      child.parent_table,
      child.table_name,
      child.table_name,
      child.parent_column,
      child.capabilities::text,
      child.parent_table,
      child.table_name,
      child.table_name,
      child.parent_column,
      child.capabilities::text
    );
  END LOOP;
END;
$$;

GRANT USAGE ON SCHEMA brunn, brunn_auth TO app_rw, app_ro;

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA brunn TO app_rw;
GRANT SELECT ON ALL TABLES IN SCHEMA brunn TO app_ro;

-- Registries are migration-owned. Unknown namespaced values are added by a
-- reviewed registry migration, not by ordinary content writes.
REVOKE INSERT, UPDATE, DELETE ON
  brunn.profile_schema_revisions,
  brunn.iana_time_zones,
  brunn.state_machines,
  brunn.state_values,
  brunn.state_transition_rules,
  brunn.relation_schema_revisions,
  brunn.relation_role_rules
FROM app_rw;

-- Account and credential creation stays behind SECURITY DEFINER bootstrap and
-- future credential-management functions. Token hashes are never selectable.
REVOKE ALL ON brunn.users FROM app_rw, app_ro;
GRANT SELECT ON brunn.users TO app_rw, app_ro;

REVOKE ALL ON brunn.scopes FROM app_rw, app_ro;
GRANT SELECT ON brunn.scopes TO app_rw, app_ro;

REVOKE ALL ON brunn.api_credentials FROM app_rw, app_ro;

REVOKE ALL ON brunn.credential_scope_grants FROM app_rw, app_ro;

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA brunn_auth FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn.ensure_profile_schema(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn.ensure_relation_schema(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.require_save_control(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.create_scope(uuid, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.issue_credential(uuid, text, text, text[], text[]) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.revoke_credential(uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.list_credentials(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION brunn.ensure_profile_schema(text) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn.ensure_relation_schema(text) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.create_scope(uuid, text, text) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.issue_credential(uuid, text, text, text[], text[]) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.revoke_credential(uuid, uuid) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.list_credentials(uuid) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.authenticate_credential(text) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.bootstrap_user(text, text, text, text, text[]) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.setting_uuid(text) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.setting_list(text) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.current_user_id() TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.current_credential_id() TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.current_capabilities() TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.current_scope_refs() TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.context_is_valid() TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.has_capability(text) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.has_any_capability(text[]) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.can_access_user(uuid) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.can_access_scope(uuid, uuid) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.can_access_optional_scope(uuid, uuid) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION brunn_auth.can_access_record(uuid, uuid) TO app_rw, app_ro;

REVOKE CREATE ON SCHEMA brunn, brunn_auth FROM app_rw, app_ro;

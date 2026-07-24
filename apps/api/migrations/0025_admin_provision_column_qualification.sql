CREATE OR REPLACE FUNCTION straylight_auth.admin_provision_user(
  p_external_ref text,
  p_display_name text,
  p_credential_label text,
  p_token_hash text,
  p_empty_manifest_hash text
)
RETURNS TABLE (
  user_id uuid,
  credential_id uuid,
  scope_id uuid,
  policy_id uuid,
  corpus_revision_id uuid
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
DECLARE
  owner_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage'
  ];
  created_user_id uuid;
  created_credential_id uuid;
  root_scope_id uuid;
  default_policy_id uuid;
  initial_revision_id uuid := gen_random_uuid();
  manifest_id uuid := gen_random_uuid();
BEGIN
  PERFORM straylight_auth.require_admin();

  IF p_external_ref IS NULL OR btrim(p_external_ref) = ''
     OR length(p_external_ref) > 200
     OR p_display_name IS NULL OR btrim(p_display_name) = ''
     OR length(p_display_name) > 200
     OR p_credential_label IS NULL OR btrim(p_credential_label) = ''
     OR length(p_credential_label) > 120 THEN
    RAISE EXCEPTION 'provisioning names are invalid'
      USING ERRCODE = '22023';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM straylight.users AS existing_user
    WHERE existing_user.external_ref = p_external_ref
  ) THEN
    RAISE EXCEPTION 'external_ref already exists'
      USING ERRCODE = '23505';
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
  )
  RETURNING api_credentials.id INTO created_credential_id;

  INSERT INTO straylight.credential_scope_grants (
    credential_id, user_id, scope_id
  ) VALUES (
    created_credential_id, created_user_id, root_scope_id
  );

  INSERT INTO straylight.corpus_revisions (
    id, user_id, scope_id, parent_revision_id, revision_number, manifest_hash
  ) VALUES (
    initial_revision_id, created_user_id, root_scope_id, NULL, 1, p_empty_manifest_hash
  );

  INSERT INTO straylight.active_manifests (
    id, user_id, scope_id, active_corpus_revision_id, manifest_hash, generation
  ) VALUES (
    manifest_id, created_user_id, root_scope_id,
    initial_revision_id, p_empty_manifest_hash, 1
  );

  INSERT INTO straylight.active_manifest_history (
    id, user_id, scope_id, manifest_id, generation, corpus_revision_id,
    manifest_hash, change_kind
  ) VALUES (
    gen_random_uuid(), created_user_id, root_scope_id, manifest_id, 1,
    initial_revision_id, p_empty_manifest_hash, 'initial'
  );

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
    created_user_id,
    created_credential_id,
    root_scope_id,
    default_policy_id,
    initial_revision_id;
END;
$$;

-- Owner credentials are the account authority and must retain every
-- capability. Restore existing owners after the temporary dreaming shutdown,
-- then prevent future partial-owner states.

DO $$
DECLARE
  owner_capabilities constant text[] := ARRAY[
    'open', 'query', 'read', 'compute', 'verify', 'status',
    'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
    'credential:manage', 'admin'
  ]::text[];
BEGIN
  WITH restored AS (
    UPDATE straylight.api_credentials AS credential
    SET capabilities = owner_capabilities
    WHERE credential.capabilities @> ARRAY['credential:manage']::text[]
      AND credential.capabilities IS DISTINCT FROM owner_capabilities
    RETURNING credential.user_id, credential.id
  )
  INSERT INTO straylight.audit_events (
    user_id, actor_ref, action, details, content_free
  )
  SELECT
    restored.user_id,
    'migration:0057',
    'auth.owner_capabilities.restore',
    jsonb_build_object(
      'credential_id', restored.id,
      'capabilities', owner_capabilities
    ),
    true
  FROM restored;
END;
$$;

ALTER TABLE straylight.api_credentials
  ADD CONSTRAINT api_credentials_owner_full_capabilities_check CHECK (
    NOT capabilities @> ARRAY['credential:manage']::text[]
    OR capabilities @> ARRAY[
      'open', 'query', 'read', 'compute', 'verify', 'status',
      'checkpoint', 'save', 'stage', 'correct', 'delete', 'dream',
      'credential:manage', 'admin'
    ]::text[]
  );

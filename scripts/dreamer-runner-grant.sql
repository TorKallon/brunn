-- Operator-only exact credential upgrade after migrations 0093..0096.
-- Render __RELEASE_REVISION__ only from a validated 40-character hex SHA.
-- No token hashes, vault values, credential identities, or scope grants change.
BEGIN;
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '15s';
DO $grant$
DECLARE
  old_caps text[];
  before_caps constant text[] := ARRAY['secret:read','secret:write','notification:publish'];
  after_caps constant text[] := ARRAY['secret:read','secret:write','notification:publish','dreamer:run'];
BEGIN
  IF (SELECT count(*) FROM _sqlx_migrations WHERE version BETWEEN 93 AND 96 AND success) <> 4 THEN
    RAISE EXCEPTION 'Dreamer migrations 0093..0096 must succeed before the grant';
  END IF;
  IF NOT EXISTS (SELECT 1 FROM brunn.api_credentials
      WHERE id='87b30ef5-8b0a-4fc6-943c-8b00563176c5'
        AND user_id='0ed8980c-d469-4e0f-857b-0c11a379b5fa'
        AND disabled_at IS NULL AND capabilities @> ARRAY['admin','credential:manage']) THEN
    RAISE EXCEPTION 'Expected active owner actor is unavailable';
  END IF;
  SELECT capabilities INTO old_caps FROM brunn.api_credentials
    WHERE id='089d1916-dfe3-454d-85cd-491a1e3e17ae'
      AND user_id='0ed8980c-d469-4e0f-857b-0c11a379b5fa' AND disabled_at IS NULL
    FOR UPDATE;
  IF old_caps IS NULL THEN
    RAISE EXCEPTION 'Expected active runner identity is unavailable';
  END IF;
  IF old_caps @> after_caps AND old_caps <@ after_caps AND cardinality(old_caps)=4 THEN
    RETURN;
  END IF;
  IF NOT (old_caps @> before_caps AND old_caps <@ before_caps AND cardinality(old_caps)=3) THEN
    RAISE EXCEPTION 'Runner capabilities changed; refusing to overwrite unexpected authority';
  END IF;
  UPDATE brunn.api_credentials SET capabilities=array_append(capabilities,'dreamer:run')
    WHERE id='089d1916-dfe3-454d-85cd-491a1e3e17ae'
      AND user_id='0ed8980c-d469-4e0f-857b-0c11a379b5fa';
  INSERT INTO brunn.audit_events(user_id,credential_id,actor_ref,action,request_id,details,content_free)
    VALUES('0ed8980c-d469-4e0f-857b-0c11a379b5fa','87b30ef5-8b0a-4fc6-943c-8b00563176c5',
      'operator:railway-dreamer-v2','auth.credential.capability_grant',
      'brunn-dreamer-v2:__RELEASE_REVISION__',
      jsonb_build_object('credential_id','089d1916-dfe3-454d-85cd-491a1e3e17ae',
        'added_capability','dreamer:run','before_capabilities',old_caps,'after_capabilities',after_caps,
        'release_revision','__RELEASE_REVISION__','transport','railway-ssh-db-local-admin'),true);
END;
$grant$;
SELECT json_build_object('credential_id',id,'owner_id',user_id,'capabilities',capabilities,
  'active',disabled_at IS NULL) FROM brunn.api_credentials
  WHERE id='089d1916-dfe3-454d-85cd-491a1e3e17ae';
COMMIT;

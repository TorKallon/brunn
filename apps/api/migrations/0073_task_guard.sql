-- Deterministic task deadline/cost guard. The worker owns scheduling; this
-- migration owns the narrow producer identity and atomic inbox/outbox write.

ALTER TABLE brunn.notifications
  DROP CONSTRAINT notifications_kind_check,
  ADD CONSTRAINT notifications_kind_check CHECK (
    kind IN (
      'briefing_ready', 'news_alert', 'correction', 'operational', 'task_guard'
    )
  );

CREATE TABLE brunn.task_guard_producers (
  user_id uuid PRIMARY KEY REFERENCES brunn.users(id) ON DELETE CASCADE,
  credential_id uuid NOT NULL UNIQUE,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES brunn.api_credentials(user_id, id) ON DELETE CASCADE
);

ALTER TABLE brunn.task_guard_producers ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.task_guard_producers FORCE ROW LEVEL SECURITY;
REVOKE ALL ON brunn.task_guard_producers FROM PUBLIC, app_rw, app_ro;

CREATE OR REPLACE FUNCTION brunn.ensure_task_guard_producer(p_user_id uuid)
RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  producer_id uuid;
BEGIN
  PERFORM pg_advisory_xact_lock(hashtextextended(
    'brunn.task-guard.producer.v1|' || p_user_id::text,
    0
  ));
  SELECT credential_id INTO producer_id
  FROM brunn.task_guard_producers
  WHERE user_id=p_user_id;
  IF producer_id IS NOT NULL THEN
    RETURN producer_id;
  END IF;

  producer_id := gen_random_uuid();
  INSERT INTO brunn.api_credentials (
    id,user_id,label,token_hash,capabilities
  ) VALUES (
    producer_id,
    p_user_id,
    '__brunn_task_guard__',
    -- This is an already-hashed, random non-bearer value. No plaintext token
    -- exists, is returned, or can be reconstructed from public identifiers.
    encode(public.gen_random_bytes(32), 'hex'),
    ARRAY['task.read','notification:publish']::text[]
  );
  INSERT INTO brunn.task_guard_producers (user_id,credential_id)
  VALUES (p_user_id,producer_id);
  RETURN producer_id;
END;
$$;

REVOKE ALL ON FUNCTION brunn.ensure_task_guard_producer(uuid)
FROM PUBLIC, app_rw, app_ro;

CREATE OR REPLACE FUNCTION brunn.seed_task_guard_producer()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
BEGIN
  PERFORM brunn.ensure_task_guard_producer(NEW.id);
  RETURN NEW;
END;
$$;

CREATE TRIGGER users_seed_task_guard_producer
AFTER INSERT ON brunn.users
FOR EACH ROW EXECUTE FUNCTION brunn.seed_task_guard_producer();

SELECT brunn.ensure_task_guard_producer(id)
FROM brunn.users;

REVOKE ALL ON FUNCTION brunn.seed_task_guard_producer()
FROM PUBLIC, app_rw, app_ro;

-- This is the only guard write primitive. It creates the inbox row even when
-- quiet hours defer transport, and inserts the delivery outbox atomically.
CREATE OR REPLACE FUNCTION brunn.enqueue_task_guard_notification(
  p_user_id uuid,
  p_task_id uuid,
  p_event_key text,
  p_title text,
  p_body text,
  p_occurred_at timestamptz,
  p_expires_at timestamptz,
  p_delivery_available_at timestamptz,
  p_delivery_enabled boolean
)
RETURNS TABLE (
  notification_id uuid,
  inserted boolean,
  delivery_count bigint
)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  producer_id uuid;
  candidate_id uuid := gen_random_uuid();
  resolved_id uuid;
  did_insert boolean;
  affected_rows bigint;
  canonical_request jsonb;
  canonical_request_hash text;
BEGIN
  IF p_user_id IS NULL
     OR p_task_id IS NULL
     OR substring(p_task_id::text from 15 for 1) <> '7'
     OR p_event_key IS NULL
     OR length(p_event_key) NOT BETWEEN 1 AND 200
     OR p_event_key !~ ('^task-(deadline|cost):' || p_task_id::text || ':')
     OR p_title IS NULL
     OR length(btrim(p_title)) NOT BETWEEN 1 AND 240
     OR p_body IS NULL
     OR length(btrim(p_body)) NOT BETWEEN 1 AND 20000
     OR p_occurred_at IS NULL
     OR p_expires_at <= p_occurred_at
     OR p_delivery_available_at IS NULL THEN
    RAISE EXCEPTION 'invalid task guard notification' USING ERRCODE='22023';
  END IF;

  IF NOT EXISTS (
    SELECT 1 FROM brunn.task_index
    WHERE user_id=p_user_id
      AND task_id=p_task_id
      AND status IN ('open','waiting')
  ) THEN
    RETURN QUERY SELECT NULL::uuid,false,0::bigint;
    RETURN;
  END IF;

  SELECT credential_id INTO producer_id
  FROM brunn.task_guard_producers
  WHERE user_id=p_user_id;
  IF producer_id IS NULL THEN
    producer_id := brunn.ensure_task_guard_producer(p_user_id);
  END IF;

  canonical_request := jsonb_build_object(
    'event_key',p_event_key,
    'kind','task_guard',
    'importance','important',
    'title',btrim(p_title),
    'body',btrim(p_body),
    'task_id',p_task_id,
    'occurred_at',p_occurred_at,
    'expires_at',p_expires_at
  );
  canonical_request_hash := encode(
    public.digest(convert_to(canonical_request::text,'UTF8'),'sha256'),
    'hex'
  );

  INSERT INTO brunn.notifications (
    id,user_id,producer_credential_id,event_key,request_hash,
    correlation_id,kind,importance,title,body,source,target,
    occurred_at,expires_at
  ) VALUES (
    candidate_id,p_user_id,producer_id,p_event_key,
    canonical_request_hash,
    'task-guard:' || p_task_id::text,
    'task_guard','important',btrim(p_title),btrim(p_body),
    jsonb_build_object('type','task_guard','ref','task:' || p_task_id::text),
    jsonb_build_object('type','task','task_ref',p_task_id::text),
    p_occurred_at,p_expires_at
  )
  ON CONFLICT (user_id,event_key) DO NOTHING;
  GET DIAGNOSTICS affected_rows = ROW_COUNT;
  did_insert := affected_rows = 1;

  SELECT id INTO resolved_id
  FROM brunn.notifications
  WHERE user_id=p_user_id AND event_key=p_event_key;

  IF NOT EXISTS (
    SELECT 1 FROM brunn.notifications AS notification
    WHERE notification.user_id=p_user_id
      AND notification.id=resolved_id
      AND notification.producer_credential_id=producer_id
      AND notification.request_hash=canonical_request_hash
      AND notification.kind='task_guard'
      AND notification.target=jsonb_build_object(
        'type','task','task_ref',p_task_id::text
      )
  ) THEN
    RAISE EXCEPTION 'task guard event key was already used with different content'
      USING ERRCODE='23505';
  END IF;

  IF did_insert THEN
    INSERT INTO brunn.notification_deliveries (
      user_id,notification_id,installation_id,state,available_at,last_error_code
    )
    SELECT p_user_id,resolved_id,installation.id,
           CASE WHEN p_delivery_enabled THEN 'queued' ELSE 'suppressed' END,
           p_delivery_available_at,
           CASE WHEN p_delivery_enabled THEN NULL ELSE 'transport_disabled' END
    FROM brunn.notification_installations AS installation
    WHERE installation.user_id=p_user_id
      AND installation.enabled
      AND installation.revoked_at IS NULL
    ON CONFLICT DO NOTHING;
  END IF;

  RETURN QUERY
  SELECT resolved_id,did_insert,count(delivery.id)
  FROM brunn.notification_deliveries AS delivery
  WHERE delivery.user_id=p_user_id
    AND delivery.notification_id=resolved_id;
END;
$$;

REVOKE ALL ON FUNCTION brunn.enqueue_task_guard_notification(
  uuid,uuid,text,text,text,timestamptz,timestamptz,timestamptz,boolean
) FROM PUBLIC, app_rw, app_ro;

-- Internal guard credentials are deliberately absent from public credential
-- inventory and cannot be revoked through its ordinary control function.
CREATE OR REPLACE FUNCTION brunn_auth.list_credentials(p_user_id uuid)
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
  SELECT credential.id,
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
    AND NOT EXISTS (
      SELECT 1 FROM brunn.web_identities AS identity
      WHERE identity.user_id = credential.user_id
        AND identity.web_credential_id = credential.id
    )
    AND NOT EXISTS (
      SELECT 1 FROM brunn.task_guard_producers AS guard
      WHERE guard.user_id=credential.user_id
        AND guard.credential_id=credential.id
    )
  GROUP BY credential.id, credential.label, credential.capabilities,
           credential.created_at, credential.disabled_at
  ORDER BY credential.created_at, credential.id;
END;
$$;

CREATE OR REPLACE FUNCTION brunn_auth.revoke_credential(
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
  PERFORM brunn_auth.require_credential_control(p_user_id);

  UPDATE brunn.api_credentials AS credential
  SET disabled_at = coalesce(credential.disabled_at, clock_timestamp())
  WHERE credential.user_id = p_user_id
    AND credential.id = p_credential_id
    AND NOT EXISTS (
      SELECT 1 FROM brunn.web_identities AS identity
      WHERE identity.user_id = credential.user_id
        AND identity.web_credential_id = credential.id
    )
    AND NOT EXISTS (
      SELECT 1 FROM brunn.task_guard_producers AS guard
      WHERE guard.user_id=credential.user_id
        AND guard.credential_id=credential.id
    )
  RETURNING credential.disabled_at INTO revoked_at;

  IF revoked_at IS NULL THEN
    RAISE EXCEPTION 'credential not found for user' USING ERRCODE = 'P0002';
  END IF;

  INSERT INTO brunn.audit_events (
    user_id, scope_id, credential_id, action, details, content_free
  ) VALUES (
    p_user_id, NULL, brunn_auth.current_credential_id(),
    'auth.credential.revoke',
    jsonb_build_object('credential_id', p_credential_id, 'revoked_at', revoked_at),
    true
  );
  RETURN revoked_at;
END;
$$;

-- A guard event is once per task/band (or ISO week), even when the task is
-- subsequently renamed or its field provenance/timestamps change. The first
-- publication is its immutable content snapshot. Replays retain that snapshot
-- and its outbox; the public producer/target preemption check still applies.
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
      AND notification.kind='task_guard'
      AND notification.target=jsonb_build_object(
        'type','task','task_ref',p_task_id::text
      )
  ) THEN
    RAISE EXCEPTION 'task guard event key belongs to a different producer or target'
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

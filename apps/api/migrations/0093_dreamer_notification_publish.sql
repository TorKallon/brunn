-- Notification publishers do not need inbox/source read access. Entry targets
-- are limited to a server-accepted exact Dreamer run version owned by this producer;
-- generic workspace writes must not assign the reserved dreamer_run metadata.
CREATE FUNCTION brunn.publisher_notification_target_allowed(
  p_target jsonb,
  p_source jsonb
)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
  SELECT COALESCE(
    brunn_auth.context_is_valid()
    AND brunn_auth.has_capability('notification:publish')
    AND p_target->>'type' = 'entry'
    AND p_target - 'type' - 'entry_ref' = '{}'::jsonb
    AND p_source->>'type' = 'dreamer_run'
    AND p_source->>'ref' = p_target->>'entry_ref'
    AND p_source - 'type' - 'ref' - 'version_ref' = '{}'::jsonb
    AND EXISTS (
      SELECT 1
      FROM brunn.entries AS entry
      JOIN brunn.entry_versions AS version
        ON version.user_id = entry.user_id
       AND version.entry_id = entry.id
      WHERE entry.user_id = brunn_auth.current_user_id()
        AND entry.id = CASE
          WHEN p_target->>'entry_ref' ~ '^entry:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
          THEN substring(p_target->>'entry_ref' FROM 7)::uuid
          ELSE NULL
        END
        AND entry.deleted_at IS NULL
        AND entry.kind = 'markdown'
        AND entry.path ~ '^dreams/runs/[0-9]{4}-[0-9]{2}-[0-9]{2}\.md$'
        AND entry.path = 'dreams/runs/' || (version.metadata->'dreamer_run'->>'date') || '.md'
        AND version.created_by_credential_id = brunn_auth.current_credential_id()
        AND version.metadata->'dreamer_run'->>'schema' = 'dream.run.v1'
        AND version.metadata->'dreamer_run'->'accepted' = 'true'::jsonb
        AND version.metadata->'dreamer_run'->>'producer_credential_id'
            = brunn_auth.current_credential_id()::text
        AND length(version.metadata->'dreamer_run'->>'attempt_id') BETWEEN 1 AND 200
        AND p_source->>'version_ref' = (p_target->>'entry_ref') || '@' || version.version::text
    ),
    false
  );
$$;

REVOKE ALL ON FUNCTION brunn.publisher_notification_target_allowed(jsonb,jsonb)
  FROM PUBLIC, app_ro;
GRANT EXECUTE ON FUNCTION brunn.publisher_notification_target_allowed(jsonb,jsonb)
  TO app_rw;

CREATE POLICY publisher_notifications_select ON brunn.notifications
FOR SELECT TO app_rw
USING (
  user_id = brunn_auth.current_user_id()
  AND brunn_auth.context_is_valid()
  AND brunn_auth.has_capability('notification:publish')
  AND producer_credential_id = brunn_auth.current_credential_id()
);

ALTER POLICY notifications_insert ON brunn.notifications
WITH CHECK (
  user_id = brunn_auth.current_user_id()
  AND brunn_auth.context_is_valid()
  AND producer_credential_id = brunn_auth.current_credential_id()
  AND (
    brunn_auth.has_capability('notification:publish')
    OR brunn_auth.has_capability('save')
    OR brunn_auth.has_capability('admin')
  )
  AND (
    brunn_auth.has_capability('read')
    OR (
      brunn_auth.has_capability('notification:publish')
      AND kind = 'operational'
      AND (
        (source IS NULL AND target = '{"type":"notification"}'::jsonb)
        OR brunn.publisher_notification_target_allowed(target,source)
      )
    )
  )
);

-- Fan-out is the only privileged installation access. The result exposes only
-- counts/status for this credential's own event; no installation identifiers,
-- encrypted tokens, delivery details, inbox contents, or other owners' data.
CREATE FUNCTION brunn.publisher_notification_fanout(
  p_notification_id uuid,
  p_enqueue boolean,
  p_delivery_enabled boolean,
  p_available_at timestamptz
)
RETURNS TABLE (delivery_count bigint, delivery_status text)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  actor_user_id uuid := brunn_auth.current_user_id();
BEGIN
  IF brunn_auth.context_is_valid() IS DISTINCT FROM true
     OR actor_user_id IS NULL
     OR brunn_auth.has_capability('notification:publish') IS DISTINCT FROM true
     OR NOT EXISTS (
       SELECT 1 FROM brunn.notifications AS notification
       WHERE notification.user_id = actor_user_id
         AND notification.id = p_notification_id
         AND notification.producer_credential_id = brunn_auth.current_credential_id()
         AND notification.kind = 'operational'
     ) THEN
    RAISE EXCEPTION 'own notification publication is required'
      USING ERRCODE = '42501';
  END IF;

  IF p_enqueue THEN
    INSERT INTO brunn.notification_deliveries (
      user_id,notification_id,installation_id,state,last_error_code,available_at
    )
    SELECT actor_user_id,p_notification_id,installation.id,
           CASE WHEN p_delivery_enabled THEN 'queued' ELSE 'suppressed' END,
           CASE WHEN p_delivery_enabled THEN NULL ELSE 'transport_disabled' END,
           COALESCE(p_available_at,clock_timestamp())
    FROM brunn.notification_installations AS installation
    WHERE installation.user_id = actor_user_id
      AND installation.enabled
      AND installation.revoked_at IS NULL
    ON CONFLICT (user_id,notification_id,installation_id) DO NOTHING;
  END IF;

  RETURN QUERY
    SELECT count(*),
      CASE
        WHEN count(*) = 0 THEN 'no_installations'
        WHEN bool_and(delivery.state = 'suppressed') THEN 'suppressed'
        WHEN bool_or(delivery.state IN ('queued','running')) THEN 'pending'
        WHEN bool_or(delivery.state IN ('failed','expired')) THEN 'failed'
        WHEN bool_and(delivery.state = 'accepted_by_apns') THEN 'accepted_by_apns'
        ELSE 'partial'
      END
    FROM brunn.notification_deliveries AS delivery
    WHERE delivery.user_id = actor_user_id
      AND delivery.notification_id = p_notification_id;
END;
$$;

REVOKE ALL ON FUNCTION brunn.publisher_notification_fanout(uuid,boolean,boolean,timestamptz)
  FROM PUBLIC, app_ro;
GRANT EXECUTE ON FUNCTION brunn.publisher_notification_fanout(uuid,boolean,boolean,timestamptz)
  TO app_rw;

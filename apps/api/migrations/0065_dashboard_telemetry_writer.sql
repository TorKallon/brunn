-- Keep asynchronous usage telemetry available to the API without giving the
-- Internet-facing process the database administrator credential. The API's
-- existing app_rw connection must establish a freshly validated transaction
-- context before calling these narrowly scoped SECURITY DEFINER writers.

CREATE FUNCTION brunn_auth.write_entry_usage(
  p_user_id uuid,
  p_credential_id uuid,
  p_entry_ids uuid[],
  p_read_counts bigint[],
  p_search_counts bigint[]
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  item_count integer;
BEGIN
  IF NOT brunn_auth.context_is_valid()
     OR brunn_auth.current_user_id() IS DISTINCT FROM p_user_id
     OR brunn_auth.current_credential_id() IS DISTINCT FROM p_credential_id THEN
    RAISE EXCEPTION 'validated exact-principal context is required for usage telemetry'
      USING ERRCODE = '42501';
  END IF;
  IF NOT coalesce((
    SELECT context.valid
    FROM brunn_auth.validate_transaction_context(
      p_user_id,
      p_credential_id,
      brunn_auth.current_capabilities(),
      brunn_auth.current_scope_refs()
    ) AS context
  ), false) THEN
    RAISE EXCEPTION 'current credential context is no longer valid for usage telemetry'
      USING ERRCODE = '42501';
  END IF;

  item_count := cardinality(p_entry_ids);
  IF p_entry_ids IS NULL
     OR p_read_counts IS NULL
     OR p_search_counts IS NULL
     OR item_count < 1
     OR item_count > 5000
     OR cardinality(p_read_counts) <> item_count
     OR cardinality(p_search_counts) <> item_count
     OR array_position(p_entry_ids, NULL) IS NOT NULL
     OR array_position(p_read_counts, NULL) IS NOT NULL
     OR array_position(p_search_counts, NULL) IS NOT NULL
     OR EXISTS (
       SELECT 1
       FROM unnest(p_read_counts, p_search_counts) AS item(read_count, search_count)
       WHERE item.read_count < 0
          OR item.search_count < 0
          OR (item.read_count = 0 AND item.search_count = 0)
     )
     OR item_count <> (
       SELECT count(DISTINCT entry_id)
       FROM unnest(p_entry_ids) AS entry_id
     ) THEN
    RAISE EXCEPTION 'entry usage telemetry arrays are invalid'
      USING ERRCODE = '22023';
  END IF;

  INSERT INTO brunn.entry_usage (
    user_id, entry_id, read_count, search_count,
    first_used_at, last_used_at, last_read_at, last_search_at
  )
  SELECT
    p_user_id,
    item.entry_id,
    item.read_count,
    item.search_count,
    clock_timestamp(),
    clock_timestamp(),
    CASE WHEN item.read_count > 0 THEN clock_timestamp() END,
    CASE WHEN item.search_count > 0 THEN clock_timestamp() END
  FROM unnest(p_entry_ids, p_read_counts, p_search_counts)
    AS item(entry_id, read_count, search_count)
  ON CONFLICT (user_id, entry_id) DO UPDATE SET
    read_count = brunn.entry_usage.read_count + EXCLUDED.read_count,
    search_count = brunn.entry_usage.search_count + EXCLUDED.search_count,
    last_used_at = clock_timestamp(),
    last_read_at = CASE WHEN EXCLUDED.read_count > 0
      THEN clock_timestamp() ELSE brunn.entry_usage.last_read_at END,
    last_search_at = CASE WHEN EXCLUDED.search_count > 0
      THEN clock_timestamp() ELSE brunn.entry_usage.last_search_at END;
END;
$$;

CREATE FUNCTION brunn_auth.write_product_activity(
  p_user_id uuid,
  p_credential_id uuid,
  p_bucket_starts timestamptz[],
  p_operations text[],
  p_operation_counts bigint[],
  p_byte_counts bigint[],
  p_first_recorded_at timestamptz[],
  p_last_recorded_at timestamptz[]
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  allowed_operations constant text[] := ARRAY[
    'open', 'search', 'read', 'binary_fetch',
    'briefing_list', 'briefing_read', 'briefing_topics',
    'write', 'capture', 'checkpoint', 'binary_upload', 'delete',
    'briefing_publish', 'briefing_action'
  ];
  item_count integer;
BEGIN
  IF NOT brunn_auth.context_is_valid()
     OR brunn_auth.current_user_id() IS DISTINCT FROM p_user_id
     OR brunn_auth.current_credential_id() IS DISTINCT FROM p_credential_id THEN
    RAISE EXCEPTION 'validated exact-principal context is required for product telemetry'
      USING ERRCODE = '42501';
  END IF;
  IF NOT coalesce((
    SELECT context.valid
    FROM brunn_auth.validate_transaction_context(
      p_user_id,
      p_credential_id,
      brunn_auth.current_capabilities(),
      brunn_auth.current_scope_refs()
    ) AS context
  ), false) THEN
    RAISE EXCEPTION 'current credential context is no longer valid for product telemetry'
      USING ERRCODE = '42501';
  END IF;

  item_count := cardinality(p_bucket_starts);
  IF p_bucket_starts IS NULL
     OR p_operations IS NULL
     OR p_operation_counts IS NULL
     OR p_byte_counts IS NULL
     OR p_first_recorded_at IS NULL
     OR p_last_recorded_at IS NULL
     OR item_count < 1
     OR item_count > 5000
     OR cardinality(p_operations) <> item_count
     OR cardinality(p_operation_counts) <> item_count
     OR cardinality(p_byte_counts) <> item_count
     OR cardinality(p_first_recorded_at) <> item_count
     OR cardinality(p_last_recorded_at) <> item_count
     OR array_position(p_bucket_starts, NULL) IS NOT NULL
     OR array_position(p_operations, NULL) IS NOT NULL
     OR array_position(p_operation_counts, NULL) IS NOT NULL
     OR array_position(p_byte_counts, NULL) IS NOT NULL
     OR array_position(p_first_recorded_at, NULL) IS NOT NULL
     OR array_position(p_last_recorded_at, NULL) IS NOT NULL
     OR NOT p_operations <@ allowed_operations
     OR EXISTS (
       SELECT 1
       FROM unnest(
         p_bucket_starts,
         p_operation_counts,
         p_byte_counts,
         p_first_recorded_at,
         p_last_recorded_at
       ) AS item(
         bucket_start,
         operation_count,
         byte_count,
         first_recorded_at,
         last_recorded_at
       )
       WHERE item.operation_count < 1
          OR item.byte_count < 0
          OR item.bucket_start <> date_trunc('minute', item.bucket_start, 'UTC')
          OR item.first_recorded_at > item.last_recorded_at
          OR item.first_recorded_at < item.bucket_start
          OR item.last_recorded_at >= item.bucket_start + interval '1 minute'
     )
     OR item_count <> (
       SELECT count(*)
       FROM (
         SELECT DISTINCT item.bucket_start, item.operation
         FROM unnest(p_bucket_starts, p_operations)
           AS item(bucket_start, operation)
       ) AS distinct_items
     ) THEN
    RAISE EXCEPTION 'product telemetry arrays are invalid'
      USING ERRCODE = '22023';
  END IF;

  INSERT INTO brunn.product_activity_minutely (
    user_id, credential_id, bucket_start, operation,
    operation_count, byte_count, first_recorded_at, last_recorded_at
  )
  SELECT
    p_user_id,
    p_credential_id,
    item.bucket_start,
    item.operation,
    item.operation_count,
    item.byte_count,
    item.first_recorded_at,
    item.last_recorded_at
  FROM unnest(
    p_bucket_starts,
    p_operations,
    p_operation_counts,
    p_byte_counts,
    p_first_recorded_at,
    p_last_recorded_at
  ) AS item(
    bucket_start,
    operation,
    operation_count,
    byte_count,
    first_recorded_at,
    last_recorded_at
  )
  ON CONFLICT (user_id, credential_id, bucket_start, operation) DO UPDATE SET
    operation_count =
      brunn.product_activity_minutely.operation_count
        + EXCLUDED.operation_count,
    byte_count =
      brunn.product_activity_minutely.byte_count + EXCLUDED.byte_count,
    first_recorded_at = least(
      brunn.product_activity_minutely.first_recorded_at,
      EXCLUDED.first_recorded_at
    ),
    last_recorded_at = greatest(
      brunn.product_activity_minutely.last_recorded_at,
      EXCLUDED.last_recorded_at
    );
END;
$$;

CREATE FUNCTION brunn_auth.write_credential_activity(
  p_user_id uuid,
  p_credential_id uuid,
  p_last_operation text,
  p_last_used_at timestamptz,
  p_request_count bigint
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn, brunn_auth
SET row_security = off
AS $$
DECLARE
  allowed_operations constant text[] := ARRAY[
    'open', 'search', 'read', 'binary_fetch',
    'briefing_list', 'briefing_read', 'briefing_topics',
    'write', 'capture', 'checkpoint', 'binary_upload', 'delete',
    'briefing_publish', 'briefing_action',
    'dashboard', 'status', 'changes',
    'credential_list', 'credential_create', 'credential_update',
    'credential_delete', 'control'
  ];
BEGIN
  IF NOT brunn_auth.context_is_valid()
     OR brunn_auth.current_user_id() IS DISTINCT FROM p_user_id
     OR brunn_auth.current_credential_id() IS DISTINCT FROM p_credential_id THEN
    RAISE EXCEPTION 'validated exact-principal context is required for credential telemetry'
      USING ERRCODE = '42501';
  END IF;
  IF NOT coalesce((
    SELECT context.valid
    FROM brunn_auth.validate_transaction_context(
      p_user_id,
      p_credential_id,
      brunn_auth.current_capabilities(),
      brunn_auth.current_scope_refs()
    ) AS context
  ), false) THEN
    RAISE EXCEPTION 'current credential context is no longer valid for credential telemetry'
      USING ERRCODE = '42501';
  END IF;

  IF p_last_operation IS NULL
     OR NOT p_last_operation = ANY(allowed_operations)
     OR p_last_used_at IS NULL
     OR p_request_count IS NULL
     OR p_request_count < 1 THEN
    RAISE EXCEPTION 'credential telemetry input is invalid'
      USING ERRCODE = '22023';
  END IF;

  INSERT INTO brunn.credential_activity (
    user_id, credential_id, last_operation, last_used_at, request_count
  ) VALUES (
    p_user_id, p_credential_id, p_last_operation, p_last_used_at, p_request_count
  )
  ON CONFLICT (user_id, credential_id) DO UPDATE SET
    last_operation = CASE
      WHEN EXCLUDED.last_used_at >= brunn.credential_activity.last_used_at
        THEN EXCLUDED.last_operation
      ELSE brunn.credential_activity.last_operation
    END,
    last_used_at = greatest(
      brunn.credential_activity.last_used_at,
      EXCLUDED.last_used_at
    ),
    request_count =
      brunn.credential_activity.request_count + EXCLUDED.request_count;
END;
$$;

REVOKE ALL ON FUNCTION brunn_auth.write_entry_usage(
  uuid, uuid, uuid[], bigint[], bigint[]
) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.write_product_activity(
  uuid, uuid, timestamptz[], text[], bigint[], bigint[], timestamptz[], timestamptz[]
) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn_auth.write_credential_activity(
  uuid, uuid, text, timestamptz, bigint
) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION brunn_auth.write_entry_usage(
  uuid, uuid, uuid[], bigint[], bigint[]
) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.write_product_activity(
  uuid, uuid, timestamptz[], text[], bigint[], bigint[], timestamptz[], timestamptz[]
) TO app_rw;
GRANT EXECUTE ON FUNCTION brunn_auth.write_credential_activity(
  uuid, uuid, text, timestamptz, bigint
) TO app_rw;

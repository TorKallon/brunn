-- Avoid PL/pgSQL record-variable shadowing in the schema-derived account purge.

CREATE OR REPLACE FUNCTION brunn.purge_account_user_rows(p_user_id uuid)
RETURNS jsonb
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, brunn
SET row_security = off
AS $$
DECLARE
  target_table record;
  deleted_rows bigint;
  remaining_rows bigint;
  result jsonb := '{}'::jsonb;
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM brunn.users
    WHERE id = p_user_id AND account_status = 'deleting'
  ) THEN
    RAISE EXCEPTION 'account purge requires a deleting user'
      USING ERRCODE = '55000';
  END IF;

  PERFORM set_config('session_replication_role', 'replica', true);
  FOR target_table IN
    SELECT column_row.table_name
    FROM information_schema.columns AS column_row
    JOIN information_schema.tables AS schema_table
      ON schema_table.table_schema = column_row.table_schema
     AND schema_table.table_name = column_row.table_name
     AND schema_table.table_type = 'BASE TABLE'
    WHERE column_row.table_schema = 'brunn'
      AND column_row.column_name = 'user_id'
      AND column_row.table_name <> ALL(ARRAY[
        'users', 'api_credentials', 'account_deletion_requests'
      ]::text[])
    ORDER BY column_row.table_name
  LOOP
    EXECUTE format(
      'DELETE FROM brunn.%I WHERE user_id=$1',
      target_table.table_name
    ) USING p_user_id;
    GET DIAGNOSTICS deleted_rows = ROW_COUNT;
    result := result || jsonb_build_object(target_table.table_name, deleted_rows);
  END LOOP;
  PERFORM set_config('session_replication_role', 'origin', true);

  FOR target_table IN
    SELECT column_row.table_name
    FROM information_schema.columns AS column_row
    JOIN information_schema.tables AS schema_table
      ON schema_table.table_schema = column_row.table_schema
     AND schema_table.table_name = column_row.table_name
     AND schema_table.table_type = 'BASE TABLE'
    WHERE column_row.table_schema = 'brunn'
      AND column_row.column_name = 'user_id'
      AND column_row.table_name <> ALL(ARRAY[
        'users', 'api_credentials', 'account_deletion_requests'
      ]::text[])
    ORDER BY column_row.table_name
  LOOP
    EXECUTE format(
      'SELECT count(*) FROM brunn.%I WHERE user_id=$1',
      target_table.table_name
    ) INTO remaining_rows USING p_user_id;
    IF remaining_rows <> 0 THEN
      RAISE EXCEPTION '% rows remain in %.% after account purge',
        remaining_rows, 'brunn', target_table.table_name
        USING ERRCODE = '55000';
    END IF;
  END LOOP;

  RETURN result;
EXCEPTION WHEN OTHERS THEN
  PERFORM set_config('session_replication_role', 'origin', true);
  RAISE;
END;
$$;

REVOKE ALL ON FUNCTION brunn.purge_account_user_rows(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION brunn.purge_account_user_rows(uuid) FROM app_rw, app_ro;

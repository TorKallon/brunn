-- The runner may use the deterministic Dreamer API without general workspace
-- write/read authority. The API scopes any internal context to this owner.
ALTER TABLE brunn.api_credentials
  DROP CONSTRAINT IF EXISTS api_credentials_capabilities_check2;
ALTER TABLE brunn.api_credentials
  ADD CONSTRAINT api_credentials_capabilities_check2 CHECK (
    capabilities <@ ARRAY[
      'open','query','read','compute','verify','status','checkpoint','save',
      'stage','correct','delete','dream','credential:manage',
      'notification:publish','notification:manage','secret:read','secret:write',
      'task.read','task.write','location.write','integration.manage',
      'message.read','message.write','dreamer:run','admin'
    ]::text[]
  );

-- A sequence is allocation-ordered, not commit-ordered. Reserve a replacement
-- generation only after taking the owner transaction fence. The identity's
-- default allocation is intentionally unused; gaps are valid, lost commits
-- are not. This covers every writer, including imports and admin workers.
CREATE OR REPLACE FUNCTION brunn.workspace_change_commit_order()
RETURNS trigger LANGUAGE plpgsql
SET search_path = pg_catalog, brunn
AS $$
BEGIN
  PERFORM pg_advisory_xact_lock(
    hashtextextended('brunn-workspace-commit:' || NEW.user_id::text, 0));
  NEW.generation := nextval(pg_get_serial_sequence('brunn.workspace_changes','generation')::regclass);
  RETURN NEW;
END;
$$;
CREATE TRIGGER workspace_changes_commit_order
BEFORE INSERT ON brunn.workspace_changes
FOR EACH ROW EXECUTE FUNCTION brunn.workspace_change_commit_order();

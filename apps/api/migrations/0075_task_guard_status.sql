-- Content-free, per-user health state for the deterministic task guard.
-- Task content and event identifiers never enter this table.

CREATE TABLE brunn.task_guard_state (
  user_id uuid PRIMARY KEY REFERENCES brunn.users(id) ON DELETE CASCADE,
  last_run_at timestamptz,
  last_outcome text CHECK (last_outcome IN ('success','failed')),
  last_error_code text CHECK (
    last_error_code IS NULL
    OR last_error_code ~ '^[a-z][a-z0-9._-]{0,119}$'
  ),
  next_run_at timestamptz,
  updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  CHECK ((last_outcome='failed') OR last_error_code IS NULL),
  CHECK ((last_run_at IS NULL) = (last_outcome IS NULL))
);

ALTER TABLE brunn.task_guard_state ENABLE ROW LEVEL SECURITY;
ALTER TABLE brunn.task_guard_state FORCE ROW LEVEL SECURITY;

CREATE POLICY task_guard_state_select
ON brunn.task_guard_state
FOR SELECT TO app_rw,app_ro
USING (
  brunn_auth.can_access_user(user_id)
  AND brunn_auth.has_any_capability(ARRAY['task.read','admin'])
);

GRANT SELECT ON brunn.task_guard_state TO app_rw,app_ro;

CREATE OR REPLACE FUNCTION brunn.seed_task_guard_state()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog,brunn
SET row_security = off
AS $$
BEGIN
  INSERT INTO brunn.task_guard_state (user_id)
  VALUES (NEW.id)
  ON CONFLICT (user_id) DO NOTHING;
  RETURN NEW;
END;
$$;

CREATE TRIGGER users_seed_task_guard_state
AFTER INSERT ON brunn.users
FOR EACH ROW EXECUTE FUNCTION brunn.seed_task_guard_state();

INSERT INTO brunn.task_guard_state (user_id)
SELECT id FROM brunn.users
ON CONFLICT (user_id) DO NOTHING;

REVOKE ALL ON FUNCTION brunn.seed_task_guard_state()
FROM PUBLIC,app_rw,app_ro;

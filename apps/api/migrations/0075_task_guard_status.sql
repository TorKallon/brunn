-- Content-free, per-user health state for the deterministic task guard.
-- Task content and event identifiers never enter this table.

CREATE TABLE straylight.task_guard_state (
  user_id uuid PRIMARY KEY REFERENCES straylight.users(id) ON DELETE CASCADE,
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

ALTER TABLE straylight.task_guard_state ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.task_guard_state FORCE ROW LEVEL SECURITY;

CREATE POLICY task_guard_state_select
ON straylight.task_guard_state
FOR SELECT TO app_rw,app_ro
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['task.read','admin'])
);

GRANT SELECT ON straylight.task_guard_state TO app_rw,app_ro;

CREATE OR REPLACE FUNCTION straylight.seed_task_guard_state()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog,straylight
SET row_security = off
AS $$
BEGIN
  INSERT INTO straylight.task_guard_state (user_id)
  VALUES (NEW.id)
  ON CONFLICT (user_id) DO NOTHING;
  RETURN NEW;
END;
$$;

CREATE TRIGGER users_seed_task_guard_state
AFTER INSERT ON straylight.users
FOR EACH ROW EXECUTE FUNCTION straylight.seed_task_guard_state();

INSERT INTO straylight.task_guard_state (user_id)
SELECT id FROM straylight.users
ON CONFLICT (user_id) DO NOTHING;

REVOKE ALL ON FUNCTION straylight.seed_task_guard_state()
FROM PUBLIC,app_rw,app_ro;

-- The application ships this cutover in the same release as the canonical
-- code. Historical migration files are reconciled by the binary immediately
-- before SQLx reaches this migration.

CREATE TEMP TABLE brunn_wave2_function_definitions (
  function_oid oid PRIMARY KEY,
  definition text NOT NULL
) ON COMMIT DROP;

DO $brunn_schema_cutover$
DECLARE
  retired_main text := concat('stray', 'light');
  retired_auth text := concat(retired_main, '_auth');
  retired_main_exists boolean;
  retired_auth_exists boolean;
  canonical_main_exists boolean;
  canonical_auth_exists boolean;
  function_row record;
  role_name text;
BEGIN
  SELECT to_regnamespace(retired_main) IS NOT NULL,
         to_regnamespace(retired_auth) IS NOT NULL,
         to_regnamespace('brunn') IS NOT NULL,
         to_regnamespace('brunn_auth') IS NOT NULL
  INTO retired_main_exists, retired_auth_exists,
       canonical_main_exists, canonical_auth_exists;

  IF retired_main_exists AND retired_auth_exists
     AND NOT canonical_main_exists AND NOT canonical_auth_exists THEN
    INSERT INTO brunn_wave2_function_definitions(function_oid, definition)
    SELECT procedure.oid,
           replace(
             replace(
               replace(
                 pg_get_functiondef(procedure.oid),
                 retired_auth,
                 'brunn_auth'
               ),
               initcap(retired_main),
               'Brunn'
             ),
             retired_main,
             'brunn'
           )
    FROM pg_proc AS procedure
    JOIN pg_namespace AS namespace ON namespace.oid=procedure.pronamespace
    WHERE namespace.nspname IN (retired_main, retired_auth)
      AND procedure.prokind IN ('f', 'p');

    EXECUTE format('ALTER SCHEMA %I RENAME TO brunn', retired_main);
    EXECUTE format('ALTER SCHEMA %I RENAME TO brunn_auth', retired_auth);
  ELSIF canonical_main_exists AND canonical_auth_exists
        AND NOT retired_main_exists AND NOT retired_auth_exists THEN
    NULL;
  ELSE
    RAISE EXCEPTION
      'schema cutover requires exactly one complete retired or canonical schema pair'
      USING ERRCODE = '55000';
  END IF;

  -- Some historical functions intentionally outlive optional relation tables.
  -- Replacing their stored definitions must preserve that valid PostgreSQL
  -- state instead of re-resolving every body during the identity-only rewrite.
  PERFORM set_config('check_function_bodies', 'off', true);

  FOR function_row IN
    SELECT definition
    FROM brunn_wave2_function_definitions
    ORDER BY function_oid
  LOOP
    EXECUTE function_row.definition;
  END LOOP;

  FOREACH role_name IN ARRAY ARRAY['app_rw', 'app_ro']
  LOOP
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname=role_name) THEN
      EXECUTE format(
        'ALTER ROLE %I SET search_path TO brunn, public',
        role_name
      );
    END IF;
  END LOOP;

  UPDATE brunn.web_sessions
  SET revoked_at=coalesce(revoked_at, clock_timestamp())
  WHERE revoked_at IS NULL;

  IF to_regnamespace(retired_main) IS NOT NULL
     OR to_regnamespace(retired_auth) IS NOT NULL
     OR to_regnamespace('brunn') IS NULL
     OR to_regnamespace('brunn_auth') IS NULL THEN
    RAISE EXCEPTION 'schema cutover did not reach the canonical pair'
      USING ERRCODE = '55000';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM pg_proc AS procedure
    JOIN pg_namespace AS namespace ON namespace.oid=procedure.pronamespace
    WHERE namespace.nspname IN ('brunn', 'brunn_auth')
      AND procedure.prokind IN ('f', 'p')
      AND position(retired_main IN lower(pg_get_functiondef(procedure.oid))) > 0
  ) THEN
    RAISE EXCEPTION 'a stored function retained the retired schema identity'
      USING ERRCODE = '55000';
  END IF;

  IF EXISTS (
    SELECT 1
    FROM pg_roles
    WHERE rolname IN ('app_rw', 'app_ro')
      AND EXISTS (
        SELECT 1
        FROM unnest(coalesce(rolconfig, ARRAY[]::text[])) AS setting
        WHERE position(retired_main IN lower(setting)) > 0
      )
  ) THEN
    RAISE EXCEPTION 'an application role retained the retired search path'
      USING ERRCODE = '55000';
  END IF;
END
$brunn_schema_cutover$;

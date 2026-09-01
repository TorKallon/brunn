-- SQLx executes each migration in a transaction unless explicitly disabled.

CREATE EXTENSION pgcrypto;
CREATE EXTENSION vector;
CREATE EXTENSION pg_trgm;
CREATE EXTENSION btree_gist;

CREATE SCHEMA brunn;
CREATE SCHEMA brunn_auth;

REVOKE ALL ON SCHEMA public FROM PUBLIC;
REVOKE ALL ON SCHEMA brunn FROM PUBLIC;
REVOKE ALL ON SCHEMA brunn_auth FROM PUBLIC;

CREATE DOMAIN brunn.nonempty_text AS text
  CHECK (length(btrim(VALUE)) > 0);

CREATE DOMAIN brunn.sha256_hex AS text
  CHECK (VALUE ~ '^[0-9a-f]{64}$');

CREATE DOMAIN brunn.namespaced_identifier AS text
  CHECK (
    VALUE ~ '^[a-z][a-z0-9_-]*(\.[a-z][a-z0-9_-]*)*(:[a-zA-Z0-9][a-zA-Z0-9._@/-]*)?$'
    OR VALUE ~ '^state:[a-z][a-z0-9_.-]*@v[1-9][0-9]*$'
  );

CREATE FUNCTION brunn.prevent_immutable_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  RAISE EXCEPTION '% is immutable; create a new revision or lineage record', TG_TABLE_NAME
    USING ERRCODE = '55000';
END;
$$;

CREATE FUNCTION brunn.prevent_physical_delete()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  RAISE EXCEPTION '% cannot be physically deleted; create a tombstone', TG_TABLE_NAME
    USING ERRCODE = '55000';
END;
$$;

CREATE FUNCTION brunn.enforce_sequential_head_advance()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF NEW.current_version <> OLD.current_version + 1 THEN
    RAISE EXCEPTION 'current_version must advance exactly once (old %, new %)',
      OLD.current_version, NEW.current_version
      USING ERRCODE = '40001';
  END IF;
  RETURN NEW;
END;
$$;


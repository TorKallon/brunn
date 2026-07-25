-- Legacy rows created before provider-version pinning may remain NULL and read
-- the latest object. Every new immutable asset and every newly published
-- upload/export must name one exact provider object version.

CREATE FUNCTION straylight.valid_object_version_id(p_value text)
RETURNS boolean
LANGUAGE sql
IMMUTABLE
PARALLEL SAFE
SET search_path = pg_catalog
AS $$
  SELECT p_value IS NOT NULL
     AND length(btrim(p_value)) > 0
     AND p_value <> 'null'
$$;

REVOKE ALL ON FUNCTION straylight.valid_object_version_id(text)
FROM PUBLIC,app_rw,app_ro;
GRANT EXECUTE ON FUNCTION straylight.valid_object_version_id(text)
TO app_rw,app_ro;

CREATE FUNCTION straylight.enforce_exact_asset_object_version()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
AS $$
BEGIN
  IF NOT straylight.valid_object_version_id(NEW.object_version_id) THEN
    RAISE EXCEPTION 'new asset versions require an exact provider object version ID'
      USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER asset_versions_require_exact_object_version
BEFORE INSERT ON straylight.asset_versions
FOR EACH ROW EXECUTE FUNCTION straylight.enforce_exact_asset_object_version();

CREATE FUNCTION straylight.enforce_published_object_versions()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
AS $$
DECLARE
  publication_requires_check boolean := false;
BEGIN
  IF NEW.temporary_object_version_id IS NOT NULL
     AND NOT straylight.valid_object_version_id(NEW.temporary_object_version_id) THEN
    RAISE EXCEPTION 'temporary object version ID is not exact'
      USING ERRCODE = '23514';
  END IF;
  IF NEW.canonical_object_version_id IS NOT NULL
     AND NOT straylight.valid_object_version_id(NEW.canonical_object_version_id) THEN
    RAISE EXCEPTION 'canonical object version ID is not exact'
      USING ERRCODE = '23514';
  END IF;
  IF NEW.status IN ('completed','consumed') THEN
    IF TG_OP = 'INSERT' THEN
      publication_requires_check := true;
    ELSE
      publication_requires_check :=
        OLD.status NOT IN ('completed','consumed')
        OR OLD.canonical_object_key IS DISTINCT FROM NEW.canonical_object_key
        OR OLD.canonical_object_version_id
             IS DISTINCT FROM NEW.canonical_object_version_id;
    END IF;
  END IF;
  IF publication_requires_check
     AND (
       NEW.canonical_object_key IS NULL
       OR NOT straylight.valid_object_version_id(
         NEW.canonical_object_version_id
       )
     ) THEN
    RAISE EXCEPTION 'published uploads require an exact canonical object version ID'
      USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER asset_uploads_require_published_object_versions
BEFORE INSERT OR UPDATE ON straylight.asset_uploads
FOR EACH ROW EXECUTE FUNCTION straylight.enforce_published_object_versions();

CREATE FUNCTION straylight.enforce_ready_export_object_version()
RETURNS trigger
LANGUAGE plpgsql
SECURITY INVOKER
AS $$
DECLARE
  publication_requires_check boolean := false;
BEGIN
  IF NEW.object_version_id IS NOT NULL
     AND NOT straylight.valid_object_version_id(NEW.object_version_id) THEN
    RAISE EXCEPTION 'account export object version ID is not exact'
      USING ERRCODE = '23514';
  END IF;
  IF NEW.status = 'ready' THEN
    IF TG_OP = 'INSERT' THEN
      publication_requires_check := true;
    ELSE
      publication_requires_check :=
        OLD.status <> 'ready'
        OR OLD.object_key IS DISTINCT FROM NEW.object_key
        OR OLD.object_version_id IS DISTINCT FROM NEW.object_version_id;
    END IF;
  END IF;
  IF publication_requires_check
     AND (
       NEW.object_key IS NULL
       OR NOT straylight.valid_object_version_id(NEW.object_version_id)
     ) THEN
    RAISE EXCEPTION 'ready account exports require an exact object version ID'
      USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER account_exports_require_exact_object_version
BEFORE INSERT OR UPDATE ON straylight.account_exports
FOR EACH ROW EXECUTE FUNCTION straylight.enforce_ready_export_object_version();

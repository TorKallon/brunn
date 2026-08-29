-- Migration 0077: suppress telemetry writes for deleting or deleted accounts.
-- The dashboard telemetry writers flush asynchronously, so a batch that lands
-- after purge_account_user_rows re-inserts usage rows for a purged user and
-- account deletion fails its retained-row verification. Telemetry for an
-- account in deletion has no consumer; drop such rows at the table boundary.

CREATE FUNCTION straylight.suppress_deleted_account_telemetry()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF EXISTS (
    SELECT 1 FROM straylight.users
    WHERE id = NEW.user_id AND account_status IN ('deleting', 'deleted')
  ) THEN
    RETURN NULL;
  END IF;
  RETURN NEW;
END;
$$;

CREATE TRIGGER entry_usage_suppress_deleted_account
BEFORE INSERT ON straylight.entry_usage
FOR EACH ROW EXECUTE FUNCTION straylight.suppress_deleted_account_telemetry();

CREATE TRIGGER product_activity_suppress_deleted_account
BEFORE INSERT ON straylight.product_activity_minutely
FOR EACH ROW EXECUTE FUNCTION straylight.suppress_deleted_account_telemetry();

CREATE TRIGGER credential_activity_suppress_deleted_account
BEFORE INSERT ON straylight.credential_activity
FOR EACH ROW EXECUTE FUNCTION straylight.suppress_deleted_account_telemetry();

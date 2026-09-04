-- Make the legacy-AAD compatibility rewrite explicit in the immutable,
-- content-free vault access history. Deploying this migration before the API
-- is rolling-compatible: older binaries continue to emit only put/get/delete.

ALTER TABLE brunn.secret_access_log
  DROP CONSTRAINT secret_access_log_operation_check,
  ADD CONSTRAINT secret_access_log_operation_check
    CHECK (operation IN ('put', 'get', 'delete', 'rewrap')) NOT VALID;

ALTER TABLE brunn.secret_access_log
  VALIDATE CONSTRAINT secret_access_log_operation_check;

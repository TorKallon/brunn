-- A retention deadline is not proof that a backup was actually pruned. Record
-- an operator-verified backup watermark and receipt before account deletion may
-- claim that retained recovery copies no longer contain the account.

ALTER TABLE straylight.account_deletion_requests
  ADD COLUMN backup_erasure_verified_at timestamptz,
  ADD COLUMN backup_erasure_watermark_at timestamptz,
  ADD COLUMN backup_erasure_receipt_sha256 text,
  ADD COLUMN backup_erasure_source text,
  ADD CONSTRAINT account_deletion_backup_erasure_proof_complete CHECK (
    (
      backup_erasure_verified_at IS NULL
      AND backup_erasure_watermark_at IS NULL
      AND backup_erasure_receipt_sha256 IS NULL
      AND backup_erasure_source IS NULL
    )
    OR (
      backup_erasure_verified_at IS NOT NULL
      AND backup_erasure_watermark_at IS NOT NULL
      AND backup_erasure_receipt_sha256 ~ '^[0-9a-f]{64}$'
      AND length(btrim(backup_erasure_source)) > 0
    )
  ),
  ADD CONSTRAINT account_deletion_completion_requires_backup_proof CHECK (
    status <> 'completed'
    OR (
      backup_erasure_verified_at IS NOT NULL
      AND backup_erasure_watermark_at IS NOT NULL
      AND backup_erasure_receipt_sha256 IS NOT NULL
      AND backup_erasure_source IS NOT NULL
      AND terminal_result ? 'canonical_purged_at'
      AND (terminal_result->>'canonical_purged_at')::timestamptz
            < backup_erasure_watermark_at
    )
  ) NOT VALID;

COMMENT ON COLUMN straylight.account_deletion_requests.backup_erasure_watermark_at IS
  'Oldest retained backup creation time proven by the referenced prune receipt.';
COMMENT ON COLUMN straylight.account_deletion_requests.backup_erasure_receipt_sha256 IS
  'SHA-256 of the operator-reviewed prune receipt that established the backup watermark.';
COMMENT ON CONSTRAINT account_deletion_completion_requires_backup_proof
  ON straylight.account_deletion_requests IS
  'Enforces verified backup erasure for new completion transitions while retaining legacy completed receipts.';

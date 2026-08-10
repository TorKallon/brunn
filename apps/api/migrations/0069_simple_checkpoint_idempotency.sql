-- A checkpoint is a single logical write. Bind its caller-supplied
-- idempotency key to the canonical request and retain the exact successful
-- receipt in the same transaction as the entry/version/change/job rows.

CREATE TABLE straylight.workspace_idempotency_receipts (
  user_id uuid NOT NULL REFERENCES straylight.users(id),
  operation_kind text NOT NULL CHECK (operation_kind = 'checkpoint'),
  idempotency_key text NOT NULL CHECK (
    char_length(idempotency_key) BETWEEN 1 AND 256
    AND idempotency_key !~ '[[:cntrl:]]'
  ),
  request_hash straylight.sha256_hex NOT NULL,
  checkpoint_entry_id uuid NOT NULL,
  pinned_workspace_generation bigint NOT NULL CHECK (
    pinned_workspace_generation >= 0
  ),
  resulting_workspace_generation bigint NOT NULL CHECK (
    resulting_workspace_generation > pinned_workspace_generation
  ),
  receipt jsonb NOT NULL CHECK (jsonb_typeof(receipt) = 'object'),
  created_by_credential_id uuid,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, operation_kind, idempotency_key),
  FOREIGN KEY (user_id, checkpoint_entry_id)
    REFERENCES straylight.entries(user_id, id),
  FOREIGN KEY (user_id, created_by_credential_id)
    REFERENCES straylight.api_credentials(user_id, id)
);

CREATE INDEX workspace_idempotency_checkpoint_entry_idx
  ON straylight.workspace_idempotency_receipts (user_id, checkpoint_entry_id);

-- This supports one-time adoption of receipts for checkpoints written by the
-- simplified endpoint before durable operation receipts existed.
CREATE INDEX entry_versions_checkpoint_idempotency_hash_idx
  ON straylight.entry_versions (
    user_id,
    ((metadata->>'_straylight_idempotency_hash'))
  )
  WHERE metadata->>'kind' = 'checkpoint'
    AND metadata ? '_straylight_idempotency_hash';

CREATE TRIGGER workspace_idempotency_receipts_immutable
BEFORE UPDATE OR DELETE ON straylight.workspace_idempotency_receipts
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

ALTER TABLE straylight.workspace_idempotency_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.workspace_idempotency_receipts FORCE ROW LEVEL SECURITY;

CREATE POLICY workspace_idempotency_receipts_select
ON straylight.workspace_idempotency_receipts
FOR SELECT TO app_rw
USING (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['checkpoint'])
);

CREATE POLICY workspace_idempotency_receipts_insert
ON straylight.workspace_idempotency_receipts
FOR INSERT TO app_rw
WITH CHECK (
  straylight_auth.can_access_user(user_id)
  AND straylight_auth.has_any_capability(ARRAY['checkpoint'])
);

GRANT SELECT, INSERT ON straylight.workspace_idempotency_receipts TO app_rw;

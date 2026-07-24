CREATE TABLE straylight.record_access_events (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  scope_id uuid NOT NULL,
  credential_id uuid NOT NULL,
  session_id uuid NOT NULL,
  corpus_revision_id uuid NOT NULL,
  projection_receipt_id uuid NOT NULL,
  target_record_id uuid NOT NULL,
  operation_kind text NOT NULL CHECK (
    operation_kind IN ('open', 'query', 'read', 'compute', 'verify')
  ),
  occurrence_count integer NOT NULL CHECK (occurrence_count > 0),
  accessed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, id),
  UNIQUE (user_id, projection_receipt_id, target_record_id),
  FOREIGN KEY (user_id, scope_id)
    REFERENCES straylight.scopes(user_id, id),
  FOREIGN KEY (user_id, credential_id)
    REFERENCES straylight.api_credentials(user_id, id),
  FOREIGN KEY (user_id, session_id)
    REFERENCES straylight.sessions(user_id, id),
  FOREIGN KEY (user_id, corpus_revision_id)
    REFERENCES straylight.corpus_revisions(user_id, id),
  FOREIGN KEY (user_id, projection_receipt_id)
    REFERENCES straylight.projection_receipts(user_id, id),
  FOREIGN KEY (user_id, target_record_id)
    REFERENCES straylight.record_keys(user_id, record_id)
);

CREATE INDEX record_access_events_target_time_idx
  ON straylight.record_access_events (
    user_id, target_record_id, accessed_at DESC
  );

CREATE INDEX record_access_events_scope_time_idx
  ON straylight.record_access_events (
    user_id, scope_id, accessed_at DESC
  );

CREATE INDEX record_access_events_operation_time_idx
  ON straylight.record_access_events (
    user_id, operation_kind, accessed_at DESC
  );

CREATE TRIGGER record_access_events_immutable
BEFORE UPDATE OR DELETE ON straylight.record_access_events
FOR EACH ROW EXECUTE FUNCTION straylight.prevent_immutable_mutation();

ALTER TABLE straylight.record_access_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE straylight.record_access_events FORCE ROW LEVEL SECURITY;

CREATE POLICY record_access_events_select
  ON straylight.record_access_events
  FOR SELECT TO app_rw, app_ro
  USING (straylight_auth.can_access_scope(user_id, scope_id));

CREATE POLICY record_access_events_insert
  ON straylight.record_access_events
  FOR INSERT TO app_rw
  WITH CHECK (
    straylight_auth.can_access_scope(user_id, scope_id)
    AND straylight_auth.has_any_capability(
      ARRAY['open', 'query', 'read', 'compute', 'verify']
    )
  );

GRANT SELECT, INSERT ON straylight.record_access_events TO app_rw;
GRANT SELECT ON straylight.record_access_events TO app_ro;

COMMENT ON TABLE straylight.record_access_events IS
  'Content-free, post-policy telemetry for records actually exposed by agent read APIs.';

COMMENT ON COLUMN straylight.record_access_events.occurrence_count IS
  'Number of model-visible references to this record in one projected response.';

-- Existing reports have unknown receipt timing. Do not backfill deployment time.
-- Ingest binds one request instant, reused through internal CAS retries, only on
-- the first committed natural-key insert. Retention still uses the original at.
ALTER TABLE brunn.location_reports ADD COLUMN first_received_at timestamptz;

ALTER TABLE brunn.deletion_jobs
  ADD COLUMN attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
  ADD COLUMN max_attempts integer NOT NULL DEFAULT 8 CHECK (max_attempts > 0),
  ADD COLUMN available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  ADD CONSTRAINT deletion_jobs_attempts_within_max_check
    CHECK (attempt_count <= max_attempts);

CREATE INDEX deletion_jobs_claim_idx
  ON brunn.deletion_jobs (available_at, created_at, id)
  WHERE status IN ('queued', 'failed', 'propagating');

COMMENT ON COLUMN brunn.deletion_jobs.attempt_count IS
  'Number of times the job has been claimed. Claims are bounded to prevent poison jobs from monopolizing the worker.';
COMMENT ON COLUMN brunn.deletion_jobs.max_attempts IS
  'Maximum worker claims before a retryable deletion failure becomes blocked and requires operator review.';
COMMENT ON COLUMN brunn.deletion_jobs.available_at IS
  'Earliest time a queued or failed deletion may be reclaimed after bounded exponential backoff.';

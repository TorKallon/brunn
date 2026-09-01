CREATE INDEX search_chunks_semantic_coverage_idx
  ON brunn.search_chunks (user_id)
  WHERE embedding IS NOT NULL;

CREATE INDEX jobs_exhausted_active_idx
  ON brunn.jobs (kind, status, started_at, id)
  WHERE status IN ('queued', 'running') AND attempts >= 5;

CREATE INDEX jobs_retryable_failed_idx
  ON brunn.jobs (kind, finished_at, id)
  WHERE status = 'failed'
    AND kind IN ('embed_entry', 'describe_binary');

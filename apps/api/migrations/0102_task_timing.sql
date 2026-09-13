-- Additive rebuildable projection. Canonical task.v1 remains the source.
ALTER TABLE brunn.task_index ADD COLUMN consequence jsonb;

-- One successor per completed native occurrence, including reopen/re-complete.
-- Stored in canonical provenance, so a projection rebuild retains the link.
CREATE UNIQUE INDEX task_native_successor_once ON brunn.task_index
  (user_id, (task->'provenance'->>'recurrence_previous_task_ref'))
  WHERE task->'provenance'->>'recurrence_previous_task_ref' IS NOT NULL;

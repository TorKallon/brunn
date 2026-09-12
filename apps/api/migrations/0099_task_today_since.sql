-- today_since: the owner-local date a task was added to the Today list.
-- Projected from the canonical task cell; persists until swept.
ALTER TABLE brunn.task_index ADD COLUMN today_since date;

CREATE INDEX task_index_today_since_idx
  ON brunn.task_index (user_id, today_since)
  WHERE today_since IS NOT NULL;

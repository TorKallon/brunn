-- Nightly project status judged by the Dreamer: a disposable daily signal on
-- the project record, not workspace content and not review state.

ALTER TABLE brunn.task_projects
  ADD COLUMN status text NOT NULL DEFAULT 'grey'
    CHECK (status IN ('grey', 'green', 'yellow', 'red')),
  ADD COLUMN status_reason text NOT NULL DEFAULT '',
  ADD COLUMN status_since date,
  ADD COLUMN status_previous text
    CHECK (status_previous IS NULL OR status_previous IN ('grey', 'green', 'yellow', 'red')),
  ADD COLUMN status_computed_on date;

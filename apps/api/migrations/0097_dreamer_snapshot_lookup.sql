-- Resolve the latest visible version of each bounded Dreamer input without
-- scanning all of the owner's history for every entry. INCLUDE(path) supplies
-- the workspace-change RLS path predicate. This preserves ordinary RLS and
-- exact frozen-generation selection; it does not add a privileged read lane.
CREATE INDEX workspace_changes_user_entry_generation_idx
ON brunn.workspace_changes (user_id, entry_id, generation DESC) INCLUDE (path);

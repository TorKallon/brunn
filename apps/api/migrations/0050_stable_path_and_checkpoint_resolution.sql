CREATE INDEX document_revisions_native_path_idx
  ON brunn.document_revisions (
    user_id,
    ((native_locator ->> 'path')),
    document_id,
    version
  )
  WHERE native_locator ? 'path';

CREATE INDEX source_episodes_source_ref_lower_idx
  ON brunn.source_episodes (
    user_id,
    scope_id,
    lower(source_ref)
  );

CREATE INDEX object_handles_handle_lower_current_idx
  ON brunn.object_handles (
    user_id,
    scope_id,
    lower(handle)
  )
  WHERE valid_to IS NULL;

CREATE INDEX object_revisions_label_lower_idx
  ON brunn.object_revisions (
    user_id,
    lower(label),
    object_id,
    version
  );

CREATE UNIQUE INDEX checkpoints_session_once_idx
  ON brunn.checkpoints (user_id, session_id);

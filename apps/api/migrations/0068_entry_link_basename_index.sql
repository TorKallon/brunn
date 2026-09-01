CREATE INDEX entries_user_normalized_basename_idx
  ON brunn.entries (
    user_id,
    (lower(normalize(regexp_replace(path, '^.*/', ''), NFC)))
  )
  WHERE deleted_at IS NULL;

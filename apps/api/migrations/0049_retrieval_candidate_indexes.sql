CREATE INDEX chunks_content_trgm_idx
  ON brunn.chunks
  USING gin (lower(content) gin_trgm_ops);

CREATE INDEX source_episodes_source_ref_fts_idx
  ON brunn.source_episodes
  USING gin (
    to_tsvector(
      'english'::regconfig,
      brunn.lexical_source_text(source_ref)
    )
  );

CREATE INDEX chunks_content_trgm_idx
  ON straylight.chunks
  USING gin (lower(content) gin_trgm_ops);

CREATE INDEX source_episodes_source_ref_fts_idx
  ON straylight.source_episodes
  USING gin (
    to_tsvector(
      'english'::regconfig,
      straylight.lexical_source_text(source_ref)
    )
  );

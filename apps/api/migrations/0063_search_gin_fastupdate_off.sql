-- The search_chunks FTS GIN index was created with the PostgreSQL default
-- fastupdate=on, so agent writes and embedding-backfill row versions
-- accumulate in the index pending list and an arbitrary later reader pays the
-- merge. That is the intermittent 1-2.5 s workspace_lexical_candidates tail
-- observed in production on 2026-08-02/03 while warm plans measured 16-24 ms
-- and the database had no memory pressure. With fastupdate off, writers
-- integrate entries directly and readers never inherit the flush; the
-- one-time clean below drains the current backlog at migration time.
ALTER INDEX straylight.search_chunks_fts_idx SET (fastupdate = off);
SELECT gin_clean_pending_list('straylight.search_chunks_fts_idx');

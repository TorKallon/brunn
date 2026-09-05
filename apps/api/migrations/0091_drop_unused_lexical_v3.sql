-- V2 is the retrieval implementation. V3 was applied but lost fallback recall
-- and has no callers; do not retain an alternative implementation.
DROP FUNCTION brunn.workspace_lexical_candidates_v3(text[],text);

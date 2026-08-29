-- Migration 0079: restore sublinear checkpoint and generation lookups.
--
-- The task and messaging RLS lanes on straylight.workspace_changes and
-- straylight.entries reference path patterns through non-leakproof
-- operators, so the planner can no longer push LIKE, expression-index, or
-- max() shortcuts past the policy barrier. max(generation) and the
-- checkpoint source/adoption lookups degraded to O(corpus) sequential
-- scans (the D09 64K regression tier caught open at ~986ms and checkpoint
-- at ~310ms against 500ms/200ms gates).
--
-- Two remedies, both following existing patterns:
-- 1. A covering index so max(generation) is an index-only backward scan;
--    the policy evaluates on index tuples and stops at the first visible
--    row.
-- 2. SECURITY DEFINER resolvers (like the search candidate functions) that
--    translate exact-principal path/hash lookups into entry id sets, which
--    the caller then uses through leakproof id=ANY() conditions under RLS.

CREATE INDEX workspace_changes_user_generation_path_idx
ON straylight.workspace_changes (user_id, generation DESC) INCLUDE (path);

-- Resolve checkpoint-source candidate paths for the authenticated user.
CREATE FUNCTION straylight_auth.resolve_entry_ids_by_path(
  p_user_id uuid,
  p_paths text[],
  p_normalized_keys text[]
)
RETURNS uuid[]
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
BEGIN
  IF NOT straylight_auth.context_is_valid()
     OR straylight_auth.current_user_id() IS DISTINCT FROM p_user_id THEN
    RAISE EXCEPTION 'validated exact-principal context is required for entry resolution'
      USING ERRCODE = '42501';
  END IF;
  RETURN coalesce((
    SELECT array_agg(id)
    FROM (
      SELECT entry.id
      FROM straylight.entries AS entry
      WHERE entry.user_id = p_user_id
        AND entry.deleted_at IS NULL
        AND entry.path = ANY(coalesce(p_paths, '{}'::text[]))
      UNION
      SELECT entry.id
      FROM straylight.entries AS entry
      WHERE entry.user_id = p_user_id
        AND entry.deleted_at IS NULL
        AND lower(normalize(entry.path, NFC)) = ANY(coalesce(p_normalized_keys, '{}'::text[]))
    ) AS matched
  ), '{}'::uuid[]);
END;
$$;

REVOKE ALL ON FUNCTION straylight_auth.resolve_entry_ids_by_path(uuid, text[], text[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight_auth.resolve_entry_ids_by_path(uuid, text[], text[]) TO app_rw;
GRANT EXECUTE ON FUNCTION straylight_auth.resolve_entry_ids_by_path(uuid, text[], text[]) TO app_ro;

-- Resolve legacy checkpoint adoption candidates by idempotency hash or exact
-- implicit path, bounded to checkpoint entries.
CREATE FUNCTION straylight_auth.resolve_checkpoint_adoption_ids(
  p_user_id uuid,
  p_idempotency_hash text,
  p_implicit_path text
)
RETURNS uuid[]
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
BEGIN
  IF NOT straylight_auth.context_is_valid()
     OR straylight_auth.current_user_id() IS DISTINCT FROM p_user_id THEN
    RAISE EXCEPTION 'validated exact-principal context is required for checkpoint adoption'
      USING ERRCODE = '42501';
  END IF;
  IF p_idempotency_hash IS NOT NULL THEN
    RETURN coalesce((
      SELECT array_agg(DISTINCT version.entry_id)
      FROM straylight.entry_versions AS version
      WHERE version.user_id = p_user_id
        AND version.metadata->>'kind' = 'checkpoint'
        AND version.metadata->>'_straylight_idempotency_hash' = p_idempotency_hash
    ), '{}'::uuid[]);
  END IF;
  RETURN coalesce((
    SELECT array_agg(entry.id)
    FROM straylight.entries AS entry
    WHERE entry.user_id = p_user_id
      AND entry.deleted_at IS NULL
      AND entry.path = p_implicit_path
  ), '{}'::uuid[]);
END;
$$;

REVOKE ALL ON FUNCTION straylight_auth.resolve_checkpoint_adoption_ids(uuid, text, text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight_auth.resolve_checkpoint_adoption_ids(uuid, text, text) TO app_rw;
GRANT EXECUTE ON FUNCTION straylight_auth.resolve_checkpoint_adoption_ids(uuid, text, text) TO app_ro;

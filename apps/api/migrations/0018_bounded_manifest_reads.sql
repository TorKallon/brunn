-- Validate the credential/scope once for bounded manifest operations. Calling
-- the RLS helper once per corpus row made cold multi-tenant reads needlessly
-- expensive while providing no additional authorization decision.

CREATE FUNCTION straylight_auth.read_manifest_stats(
  p_user_id uuid,
  p_scope_id uuid,
  p_corpus_revision_id uuid
)
RETURNS TABLE(record_count bigint, chunk_tokens bigint)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
BEGIN
  IF NOT straylight_auth.can_access_scope(p_user_id, p_scope_id) THEN
    RETURN;
  END IF;

  RETURN QUERY
  SELECT count(*)::bigint,
         coalesce(sum(chunk.token_count::bigint) FILTER (
           WHERE member.record_kind = 'chunk'
         ), 0)::bigint
  FROM straylight.corpus_members AS member
  LEFT JOIN straylight.chunks AS chunk
    ON chunk.user_id = member.user_id
   AND chunk.id = member.record_id
  WHERE member.user_id = p_user_id
    AND member.scope_id = p_scope_id
    AND member.corpus_revision_id = p_corpus_revision_id
    AND member.disposition = 'active';
END
$$;

CREATE FUNCTION straylight_auth.read_manifest_sample(
  p_user_id uuid,
  p_scope_id uuid,
  p_corpus_revision_id uuid,
  p_sample_limit integer
)
RETURNS TABLE(
  record_kind text,
  record_id uuid,
  record_version integer,
  disposition text,
  kind_count bigint
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, straylight, straylight_auth
SET row_security = off
AS $$
BEGIN
  IF NOT straylight_auth.can_access_scope(p_user_id, p_scope_id) THEN
    RETURN;
  END IF;

  RETURN QUERY
  WITH ranked AS (
    SELECT member.record_kind::text AS record_kind,
           member.record_id,
           member.record_version,
           member.disposition::text AS disposition,
           count(*) OVER (PARTITION BY member.record_kind)::bigint AS kind_count,
           row_number() OVER (
             PARTITION BY member.record_kind ORDER BY member.record_id
           ) AS sample_position
    FROM straylight.corpus_members AS member
    WHERE member.user_id = p_user_id
      AND member.scope_id = p_scope_id
      AND member.corpus_revision_id = p_corpus_revision_id
  )
  SELECT ranked.record_kind,
         ranked.record_id,
         ranked.record_version,
         ranked.disposition,
         ranked.kind_count
  FROM ranked
  WHERE ranked.sample_position <= greatest(p_sample_limit, 0)
  ORDER BY ranked.record_kind, ranked.record_id;
END
$$;

REVOKE ALL ON FUNCTION straylight_auth.read_manifest_stats(uuid, uuid, uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION straylight_auth.read_manifest_sample(uuid, uuid, uuid, integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION straylight_auth.read_manifest_stats(uuid, uuid, uuid) TO app_rw, app_ro;
GRANT EXECUTE ON FUNCTION straylight_auth.read_manifest_sample(uuid, uuid, uuid, integer) TO app_rw, app_ro;

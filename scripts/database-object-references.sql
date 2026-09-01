\pset tuples_only on
\pset format unaligned

WITH authoritative_candidates AS (
  SELECT
    'asset_versions'::text AS relation,
    user_id,
    bucket,
    object_key,
    object_version_id,
    'sha256:' || content_hash AS content_hash,
    size_bytes
  FROM brunn.asset_versions
  WHERE NOT (
    object_key::text = user_id::text || '/deleted-assets/'
      || asset_id::text || '/' || version::text
    AND content_hash::text =
      'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
    AND size_bytes = 0
    AND media_type::text = 'application/x-brunn-deleted'
    AND metadata ? 'redacted_by_deletion_job'
  )
  UNION ALL

  SELECT
    'account_exports',
    user_id,
    current_setting('brunn.backup_object_bucket'),
    object_key,
    object_version_id,
    'sha256:' || content_hash,
    size_bytes
  FROM brunn.account_exports
  WHERE status = 'ready'
    AND object_key IS NOT NULL

  UNION ALL

  SELECT
    'entry_versions',
    user_id,
    current_setting('brunn.backup_object_bucket'),
    object_key,
    object_version_id,
    'sha256:' || content_sha256,
    size_bytes
  FROM brunn.entry_versions
  WHERE object_key IS NOT NULL
),
authoritative_references AS (
  SELECT *
  FROM authoritative_candidates
  WHERE object_version_id IS NOT NULL
    AND content_hash IS NOT NULL
    AND size_bytes IS NOT NULL
),
ordered_references AS (
  SELECT *
  FROM authoritative_references
  ORDER BY relation, user_id, object_key, object_version_id
)
SELECT jsonb_build_object(
  'format', 'brunn-database-object-references@v1',
  'object_bucket', current_setting('brunn.backup_object_bucket'),
  'references',
    coalesce(
      jsonb_agg(
        jsonb_build_object(
          'relation', relation,
          'user_id', user_id,
          'bucket', bucket,
          'object_key', object_key,
          'object_version_id', object_version_id,
          'content_hash', content_hash,
          'size_bytes', size_bytes
        )
      ),
      '[]'::jsonb
    ),
  'summary',
    jsonb_build_object(
      'reference_count', count(*),
      'distinct_object_versions',
        count(DISTINCT (object_key, object_version_id)),
      'authoritative_candidate_count',
        (SELECT count(*) FROM authoritative_candidates),
      'unresolved_reference_count',
        (
          SELECT count(*)
          FROM authoritative_candidates
          WHERE object_version_id IS NULL
             OR content_hash IS NULL
             OR size_bytes IS NULL
        )
    )
)
FROM ordered_references;

\pset tuples_only on
\pset format unaligned
\pset fieldsep '|'

SELECT format(
  'SELECT %L, count(*)::bigint FROM %I.%I;',
  schemaname || '.' || tablename,
  schemaname,
  tablename
)
FROM pg_tables
WHERE schemaname IN ('brunn', 'brunn_auth')
ORDER BY schemaname, tablename
\gexec

SELECT '_sqlx_migrations.max_version', coalesce(max(version), 0)
FROM public._sqlx_migrations;

SELECT '_sqlx_migrations.count', count(*)
FROM public._sqlx_migrations;

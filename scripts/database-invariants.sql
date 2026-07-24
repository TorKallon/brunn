\pset tuples_only on
\pset format unaligned

SELECT jsonb_build_object(
  'database', current_database(),
  'server_version_num', current_setting('server_version_num')::integer,
  'data_checksums', current_setting('data_checksums'),
  'locale_provider', database_row.datlocprovider,
  'locale', database_row.datlocale,
  'recorded_collation_version', database_row.datcollversion,
  'actual_collation_version',
    pg_database_collation_actual_version(database_row.oid),
  'safe',
    database_row.datlocprovider = 'b'
    AND database_row.datlocale = 'C.UTF-8'
    AND database_row.datcollversion =
      pg_database_collation_actual_version(database_row.oid)
    AND current_setting('data_checksums') = 'on'
)
FROM pg_database AS database_row
WHERE database_row.datname = current_database();

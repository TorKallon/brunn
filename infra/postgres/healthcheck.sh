#!/bin/sh
set -eu

database=${POSTGRES_DB:-straylight}
user=${POSTGRES_USER:-admin}

pg_isready --username "$user" --dbname "$database" >/dev/null

status=$(
  psql \
    --username "$user" \
    --dbname "$database" \
    --no-psqlrc \
    --tuples-only \
    --no-align \
    --set ON_ERROR_STOP=1 \
    --command "
      SELECT CASE
        WHEN datlocprovider = 'b'
          AND datlocale = 'C.UTF-8'
          AND datcollversion = pg_database_collation_actual_version(oid)
          AND current_setting('data_checksums') = 'on'
        THEN 'ready'
        ELSE 'unsafe'
      END
      FROM pg_database
      WHERE datname = current_database()
    " |
    tr -d '[:space:]'
)

if [ "$status" != "ready" ]; then
  echo "database requires builtin C.UTF-8 collation and page checksums" >&2
  exit 1
fi

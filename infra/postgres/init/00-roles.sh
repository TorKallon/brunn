#!/bin/sh
set -eu

psql --set=ON_ERROR_STOP=1 \
  --username "$POSTGRES_USER" \
  --dbname "$POSTGRES_DB" \
  --set=db_name="$POSTGRES_DB" \
  --set=rw_password="$APP_RW_PASSWORD" \
  --set=ro_password="$APP_RO_PASSWORD" <<'SQL'
ALTER ROLE admin WITH LOGIN SUPERUSER CREATEDB CREATEROLE INHERIT BYPASSRLS;

SELECT format(
  'CREATE ROLE app_rw LOGIN PASSWORD %L NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS',
  :'rw_password'
)
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'app_rw')
\gexec

SELECT format(
  'CREATE ROLE app_ro LOGIN PASSWORD %L NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS',
  :'ro_password'
)
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'app_ro')
\gexec

SELECT format('ALTER ROLE app_rw PASSWORD %L', :'rw_password')
\gexec
SELECT format('ALTER ROLE app_ro PASSWORD %L', :'ro_password')
\gexec

ALTER ROLE app_rw SET search_path = straylight, public;
ALTER ROLE app_ro SET search_path = straylight, public;

SELECT format('REVOKE ALL ON DATABASE %I FROM PUBLIC', :'db_name')
\gexec
SELECT format('GRANT CONNECT ON DATABASE %I TO app_rw, app_ro', :'db_name')
\gexec
SQL

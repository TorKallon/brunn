#!/bin/sh
set -eu

load_secret() {
  variable=$1
  file_variable="${variable}_FILE"
  eval "value=\${${variable}:-}"
  eval "file=\${${file_variable}:-}"
  if [ -n "$value" ] && [ -n "$file" ]; then
    echo "$variable and $file_variable cannot both be set" >&2
    exit 1
  fi
  if [ -n "$file" ]; then
    value=$(cat "$file")
    export "$variable=$value"
  fi
  [ -n "$value" ] || {
    echo "$variable is required" >&2
    exit 1
  }
}

load_secret APP_RW_PASSWORD
load_secret APP_RO_PASSWORD

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

ALTER ROLE app_rw SET search_path = brunn, public;
ALTER ROLE app_ro SET search_path = brunn, public;

SELECT format('REVOKE ALL ON DATABASE %I FROM PUBLIC', :'db_name')
\gexec
SELECT format('GRANT CONNECT ON DATABASE %I TO app_rw, app_ro', :'db_name')
\gexec
SQL

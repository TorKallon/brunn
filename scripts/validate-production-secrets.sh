#!/bin/sh
set -eu

usage() {
  echo "usage: $0 SECRETS_DIR" >&2
  exit 64
}

[ "$#" -eq 1 ] || usage
secrets_dir=$1
[ -d "$secrets_dir" ] || {
  echo "production secrets directory does not exist: $secrets_dir" >&2
  exit 1
}
[ ! -L "$secrets_dir" ] || {
  echo "production secrets directory must not be a symbolic link: $secrets_dir" >&2
  exit 1
}
directory_mode=$(stat -f '%Lp' "$secrets_dir" 2>/dev/null || stat -c '%a' "$secrets_dir")
[ "$directory_mode" = "700" ] || {
  echo "production secrets directory must have mode 0700: $secrets_dir (mode $directory_mode)" >&2
  exit 1
}
directory_owner=$(stat -f '%u' "$secrets_dir" 2>/dev/null || stat -c '%u' "$secrets_dir")
[ "$directory_owner" = "$(id -u)" ] || {
  echo "production secrets directory must be owned by the current operator: $secrets_dir" >&2
  exit 1
}

required="
postgres_admin_password
postgres_app_rw_password
postgres_app_ro_password
database_url_rw
database_url_ro
database_url_admin
minio_root_user
minio_root_password
minio_app_access_key
minio_app_secret_key
continuation_signing_key
openai_api_key
dd_api_key
"

count=0
for name in $required; do
  path="$secrets_dir/$name"
  [ -f "$path" ] && [ ! -L "$path" ] || {
    echo "missing regular secret file: $path" >&2
    exit 1
  }
  [ -s "$path" ] || {
    echo "secret file is empty: $path" >&2
    exit 1
  }
  mode=$(stat -f '%Lp' "$path" 2>/dev/null || stat -c '%a' "$path")
  case "$mode" in
    400|600)
      ;;
    *)
      echo "secret file must have mode 0400 or 0600: $path (mode $mode)" >&2
      exit 1
      ;;
  esac
  count=$((count + 1))
done

require_minimum_bytes() {
  name=$1
  minimum=$2
  length=$(wc -c <"$secrets_dir/$name" | tr -d '[:space:]')
  [ "$length" -ge "$minimum" ] || {
    echo "$name must contain at least $minimum bytes" >&2
    exit 1
  }
}

require_minimum_bytes postgres_admin_password 16
require_minimum_bytes postgres_app_rw_password 16
require_minimum_bytes postgres_app_ro_password 16
require_minimum_bytes minio_root_password 16
require_minimum_bytes minio_app_secret_key 16
require_minimum_bytes continuation_signing_key 32
require_minimum_bytes openai_api_key 20
require_minimum_bytes dd_api_key 20

for name in postgres_admin_password postgres_app_rw_password \
  postgres_app_ro_password minio_root_password minio_app_secret_key \
  continuation_signing_key openai_api_key dd_api_key; do
  if grep -Eiq '(^|[_-])(replace|change-?me|placeholder|example)([_-]|$)' \
    "$secrets_dir/$name"; then
    echo "$name contains a placeholder value" >&2
    exit 1
  fi
done

for name in database_url_rw database_url_ro database_url_admin; do
  first_line=$(sed -n '1p' "$secrets_dir/$name")
  case "$first_line" in
    postgres://*|postgresql://*)
      ;;
    *)
      echo "$name must contain a PostgreSQL connection URL" >&2
      exit 1
      ;;
  esac
done

case "$(sed -n '1p' "$secrets_dir/database_url_rw")" in
  postgres://app_rw:*@db:5432/*|postgresql://app_rw:*@db:5432/*)
    ;;
  *)
    echo "database_url_rw must use app_rw on the internal db service" >&2
    exit 1
    ;;
esac
case "$(sed -n '1p' "$secrets_dir/database_url_ro")" in
  postgres://app_ro:*@db:5432/*|postgresql://app_ro:*@db:5432/*)
    ;;
  *)
    echo "database_url_ro must use app_ro on the internal db service" >&2
    exit 1
    ;;
esac
case "$(sed -n '1p' "$secrets_dir/database_url_admin")" in
  postgres://admin:*@db:5432/*|postgresql://admin:*@db:5432/*)
    ;;
  *)
    echo "database_url_admin must use admin on the internal db service" >&2
    exit 1
    ;;
esac

echo "production secret contract valid: files=$count directory=$secrets_dir"

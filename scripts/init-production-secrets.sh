#!/bin/sh
set -eu
umask 077

usage() {
  echo "usage: $0 SECRETS_DIR OPENAI_KEY_FILE RESEND_KEY_FILE DATADOG_KEY_FILE APNS_PRIVATE_KEY_FILE" >&2
  exit 64
}

[ "$#" -eq 5 ] || usage
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
secrets_dir=$1
openai_source=$2
resend_source=$3
datadog_source=$4
apns_private_key_source=$5

[ ! -e "$secrets_dir" ] || {
  echo "refusing to replace an existing secrets path: $secrets_dir" >&2
  exit 1
}
for source in "$openai_source" "$resend_source" "$datadog_source" \
  "$apns_private_key_source"; do
  [ -f "$source" ] && [ ! -L "$source" ] && [ -s "$source" ] || {
    echo "account key source must be a nonempty regular file: $source" >&2
    exit 1
  }
done

parent=$(CDPATH= cd -- "$(dirname "$secrets_dir")" && pwd)
name=$(basename "$secrets_dir")
work_dir="$parent/.$name.partial.$$"
mkdir -m 0700 "$work_dir"

cleanup() {
  status=$?
  trap - EXIT INT TERM
  if [ "$status" -ne 0 ]; then
    rm -rf "$work_dir"
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

random_hex() {
  bytes=$1
  od -An -N "$bytes" -tx1 /dev/urandom | tr -d '[:space:]'
}

random_base64() {
  bytes=$1
  dd if=/dev/urandom bs="$bytes" count=1 2>/dev/null | base64 | tr -d '\r\n'
}

write_secret() {
  name=$1
  value=$2
  printf '%s' "$value" >"$work_dir/$name"
  chmod 0600 "$work_dir/$name"
}

read_account_key() {
  source=$1
  value=$(cat "$source")
  [ -n "$value" ] || {
    echo "account key source is empty: $source" >&2
    exit 1
  }
  printf '%s' "$value"
}

postgres_admin_password=$(random_hex 32)
postgres_app_rw_password=$(random_hex 32)
postgres_app_ro_password=$(random_hex 32)
minio_root_user="straylight-root-$(random_hex 8)"
minio_root_password=$(random_hex 32)
minio_app_access_key="straylight-app-$(random_hex 8)"
minio_app_secret_key=$(random_hex 32)

write_secret postgres_admin_password "$postgres_admin_password"
write_secret postgres_app_rw_password "$postgres_app_rw_password"
write_secret postgres_app_ro_password "$postgres_app_ro_password"
write_secret database_url_rw \
  "postgres://app_rw:$postgres_app_rw_password@db:5432/straylight"
write_secret database_url_ro \
  "postgres://app_ro:$postgres_app_ro_password@db:5432/straylight"
write_secret database_url_admin \
  "postgres://admin:$postgres_admin_password@db:5432/straylight"
write_secret minio_root_user "$minio_root_user"
write_secret minio_root_password "$minio_root_password"
write_secret minio_app_access_key "$minio_app_access_key"
write_secret minio_app_secret_key "$minio_app_secret_key"
write_secret continuation_signing_key "$(random_hex 32)"
write_secret notification_token_encryption_key "$(random_base64 32)"
write_secret secret_encryption_key "$(random_base64 32)"
write_secret apns_private_key "$(read_account_key "$apns_private_key_source")"
write_secret openai_api_key "$(read_account_key "$openai_source")"
write_secret resend_api_key "$(read_account_key "$resend_source")"
write_secret dd_api_key "$(read_account_key "$datadog_source")"

"$root/scripts/validate-production-secrets.sh" "$work_dir" >/dev/null
mv "$work_dir" "$secrets_dir"
trap - EXIT INT TERM
echo "production secrets initialized: $secrets_dir"

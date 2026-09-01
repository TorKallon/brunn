#!/bin/sh
set -eu

usage() {
  echo "usage: $0 ENV_FILE" >&2
  exit 64
}

[ "$#" -eq 1 ] || usage
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
env_file=$1
[ -f "$env_file" ] || {
  echo "production environment file does not exist: $env_file" >&2
  exit 1
}

read_value() {
  awk -F= -v key="$1" '
    $1 == key {
      sub(/^[^=]*=/, "")
      print
      exit
    }
  ' "$env_file"
}

require_value() {
  name=$1
  value=$(read_value "$name")
  [ -n "$value" ] || {
    echo "$name must be set in $env_file" >&2
    exit 1
  }
  printf '%s' "$value"
}

environment=$(require_value BRUNN_ENV)
[ "$environment" = "production" ] || {
  echo "BRUNN_ENV must be production" >&2
  exit 1
}

object_store_mode=$(read_value BRUNN_OBJECT_STORE_MODE)
object_store_mode=${object_store_mode:-self-hosted-minio}
case "$object_store_mode" in
  self-hosted-minio|managed-s3)
    ;;
  *)
    echo "BRUNN_OBJECT_STORE_MODE must be self-hosted-minio or managed-s3" >&2
    exit 1
    ;;
esac

release_revision=$(require_value BRUNN_RELEASE_REVISION)
case "$release_revision" in
  *[!0-9a-f]*)
    echo "BRUNN_RELEASE_REVISION must be a lowercase 40-character Git commit" >&2
    exit 1
    ;;
esac
[ "${#release_revision}" -eq 40 ] || {
  echo "BRUNN_RELEASE_REVISION must be a lowercase 40-character Git commit" >&2
  exit 1
}
[ "$release_revision" != "0000000000000000000000000000000000000000" ] || {
  echo "BRUNN_RELEASE_REVISION must not be the template revision" >&2
  exit 1
}

dd_version=$(require_value DD_VERSION)
[ "$dd_version" = "$release_revision" ] || {
  echo "DD_VERSION must equal BRUNN_RELEASE_REVISION" >&2
  exit 1
}
[ "$(require_value DD_ENV)" = "production" ] || {
  echo "DD_ENV must be production" >&2
  exit 1
}

require_exact() {
  name=$1
  expected=$2
  actual=$(require_value "$name")
  [ "$actual" = "$expected" ] || {
    echo "$name must remain $expected for this release contract" >&2
    exit 1
  }
}

require_exact BRUNN_EMBEDDING_PROVIDER openai
require_exact BRUNN_ALLOW_DEGRADED_EMBEDDINGS false
require_exact BRUNN_EMBEDDING_MODEL text-embedding-3-small
require_exact BRUNN_EMBEDDING_DIMENSIONS 1536
require_exact BRUNN_CAPTURE_MODEL gpt-5.6
require_exact BRUNN_CAPTURE_MAX_OUTPUT_TOKENS 8192
require_exact BRUNN_DREAM_MODEL gpt-5.6
require_exact BRUNN_MATERIALIZE_TOKEN_BUDGET 24000
require_exact OPENAI_BASE_URL https://api.openai.com/v1
require_exact BRUNN_DREAM_SCHEDULER_ENABLED false
require_exact BRUNN_METRICS_ENABLED true
require_exact BRUNN_DOGSTATSD_ADDR datadog-agent:8125

require_exact BRUNN_APNS_APP_ID com.rourkem.brunn

apns_delivery_enabled=$(read_value BRUNN_APNS_DELIVERY_ENABLED)
apns_delivery_enabled=${apns_delivery_enabled:-false}
case "$apns_delivery_enabled" in
  true|false)
    ;;
  *)
    echo "BRUNN_APNS_DELIVERY_ENABLED must be true or false" >&2
    exit 1
    ;;
esac

messaging_enabled=$(read_value BRUNN_MESSAGING_ENABLED)
messaging_enabled=${messaging_enabled:-false}
case "$messaging_enabled" in
  true|false)
    ;;
  *)
    echo "BRUNN_MESSAGING_ENABLED must be true or false" >&2
    exit 1
    ;;
esac

for apns_id_name in BRUNN_APNS_TEAM_ID BRUNN_APNS_KEY_ID; do
  apns_id=$(require_value "$apns_id_name")
  case "$apns_id" in
    *[!A-Za-z0-9]*|*replace*|*example*|*placeholder*)
      echo "$apns_id_name must be a non-placeholder 10-character Apple identifier" >&2
      exit 1
      ;;
  esac
  [ "${#apns_id}" -eq 10 ] || {
    echo "$apns_id_name must be a non-placeholder 10-character Apple identifier" >&2
    exit 1
  }
done

require_uint_between() {
  name=$1
  minimum=$2
  maximum=$3
  value=$(require_value "$name")
  case "$value" in
    ''|*[!0-9]*)
      echo "$name must be an integer between $minimum and $maximum" >&2
      exit 1
      ;;
  esac
  [ "$value" -ge "$minimum" ] && [ "$value" -le "$maximum" ] || {
    echo "$name must be between $minimum and $maximum" >&2
    exit 1
  }
  printf '%s' "$value"
}

require_uint_between BRUNN_REQUESTS_PER_MINUTE 60 6000 >/dev/null
require_uint_between BRUNN_REQUEST_TIMEOUT_SECONDS 5 120 >/dev/null
require_uint_between BRUNN_TRANSFER_TIMEOUT_SECONDS 300 7200 >/dev/null
require_uint_between BRUNN_MAX_CONCURRENT_TRANSFERS 1 32 >/dev/null
require_uint_between BRUNN_READINESS_TIMEOUT_SECONDS 1 15 >/dev/null
require_uint_between BRUNN_METRICS_FLUSH_SECONDS 1 30 >/dev/null
backup_retention=$(require_uint_between BRUNN_BACKUP_RETENTION_DAYS 1 90)
deletion_retention=$(
  require_uint_between BRUNN_ACCOUNT_DELETION_BACKUP_RETENTION_DAYS 1 90
)
[ "$backup_retention" -le "$deletion_retention" ] || {
  echo "BRUNN_BACKUP_RETENTION_DAYS must not exceed account-deletion backup retention" >&2
  exit 1
}

notify=$(require_value BRUNN_DATADOG_NOTIFY)
case "$notify" in
  *replace*|*example*|*@example.com|*@example.net|*@example.org)
    echo "BRUNN_DATADOG_NOTIFY must contain an approved non-placeholder destination" >&2
    exit 1
    ;;
esac

[ -z "$(read_value BRUNN_ALLOWED_ORIGINS)" ] || {
  echo "BRUNN_ALLOWED_ORIGINS must remain empty for the same-origin production SPA" >&2
  exit 1
}

public_host=$(require_value BRUNN_PUBLIC_HOST)
case "$public_host" in
  memory.example.com|*.example.com|*.example.net|*.example.org|*.invalid|*.test|localhost|*.localhost|*://*|*/*|*:*|*[!A-Za-z0-9.-]*)
    echo "BRUNN_PUBLIC_HOST must be an approved DNS hostname without a scheme, port, path, or placeholder domain" >&2
    exit 1
    ;;
esac
case "$public_host" in
  .*|*.|*..*)
    echo "BRUNN_PUBLIC_HOST is not a valid DNS hostname" >&2
    exit 1
    ;;
esac
case "$public_host" in
  *.*)
    ;;
  *)
    echo "BRUNN_PUBLIC_HOST must be a fully qualified DNS hostname" >&2
    exit 1
    ;;
esac

public_url=$(require_value BRUNN_PUBLIC_URL)
[ "$public_url" = "https://$public_host" ] || {
  echo "BRUNN_PUBLIC_URL must equal https://BRUNN_PUBLIC_HOST" >&2
  exit 1
}

auth_email_from=$(require_value AUTH_EMAIL_FROM)
case "$auth_email_from" in
  *@*.*)
    ;;
  *)
    echo "AUTH_EMAIL_FROM must contain a valid sender email address" >&2
    exit 1
    ;;
esac
case "$auth_email_from" in
  *example.com*|*example.net*|*example.org*|*replace*|*placeholder*)
    echo "AUTH_EMAIL_FROM must not use a placeholder sender" >&2
    exit 1
    ;;
esac

auth_email_reply_to=$(read_value AUTH_EMAIL_REPLY_TO)
case "$auth_email_reply_to" in
  ""|*@*.*)
    ;;
  *)
    echo "AUTH_EMAIL_REPLY_TO must be empty or a valid email address" >&2
    exit 1
    ;;
esac

acme_email=$(require_value BRUNN_ACME_EMAIL)
case "$acme_email" in
  *@*.*)
    ;;
  *)
    echo "BRUNN_ACME_EMAIL must be a valid certificate-notification address" >&2
    exit 1
    ;;
esac
case "$acme_email" in
  *@example.com|*@example.net|*@example.org)
    echo "BRUNN_ACME_EMAIL must not use a placeholder domain" >&2
    exit 1
    ;;
esac

datadog_image=$(require_value DATADOG_AGENT_IMAGE)
validate_digest_image() {
  name=$1
  image=$2
  case "$image" in
    *replace*|*example*)
      echo "$name must not use a placeholder image reference" >&2
      exit 1
      ;;
  esac
  case "$image" in
    *@sha256:*)
      digest=${image##*@sha256:}
      ;;
    *)
      digest=
      ;;
  esac
  case "$digest" in
    *[!0-9a-f]*)
      digest=
      ;;
  esac
  [ "${#digest}" -eq 64 ] || {
    echo "$name must be pinned by sha256 digest" >&2
    exit 1
  }
  [ "$digest" != "0000000000000000000000000000000000000000000000000000000000000000" ] || {
    echo "$name must not use the template digest" >&2
    exit 1
  }
}

validate_release_image() {
  name=$1
  image=$(require_value "$name")
  validate_digest_image "$name" "$image"
}

validate_digest_image DATADOG_AGENT_IMAGE "$datadog_image"
validate_digest_image BRUNN_DATABASE_IMAGE \
  "$(require_value BRUNN_DATABASE_IMAGE)"
if [ "$object_store_mode" = "self-hosted-minio" ]; then
  validate_digest_image BRUNN_OBJECT_STORE_IMAGE \
    "$(require_value BRUNN_OBJECT_STORE_IMAGE)"
  validate_digest_image BRUNN_OBJECT_STORE_CLIENT_IMAGE \
    "$(require_value BRUNN_OBJECT_STORE_CLIENT_IMAGE)"
else
  s3_region=$(require_value BRUNN_S3_REGION)
  s3_bucket=$(require_value BRUNN_S3_BUCKET)
  case "$s3_region:$s3_bucket" in
    *replace*|*example*|*placeholder*)
      echo "managed S3 region and bucket must not contain placeholders" >&2
      exit 1
      ;;
  esac
  require_exact BRUNN_S3_CREATE_BUCKET false
  s3_path_style=$(require_value BRUNN_S3_FORCE_PATH_STYLE)
  case "$s3_path_style" in
    true|false)
      ;;
    *)
      echo "BRUNN_S3_FORCE_PATH_STYLE must be true or false" >&2
      exit 1
      ;;
  esac
  s3_endpoint=$(read_value BRUNN_S3_ENDPOINT)
  case "$s3_endpoint" in
    "")
      ;;
    http://*|https://*)
      ;;
    *)
      echo "BRUNN_S3_ENDPOINT must be empty or an HTTP(S) endpoint" >&2
      exit 1
      ;;
  esac
  s3_access_key=$(read_value BRUNN_S3_ACCESS_KEY)
  s3_secret_key=$(read_value BRUNN_S3_SECRET_KEY)
  if [ -n "$s3_access_key" ] || [ -n "$s3_secret_key" ]; then
    echo "managed S3 direct keys must be empty; use both _FILE settings or workload identity" >&2
    exit 1
  fi
  s3_access_key_file=$(read_value BRUNN_S3_ACCESS_KEY_FILE)
  s3_secret_key_file=$(read_value BRUNN_S3_SECRET_KEY_FILE)
  if { [ -n "$s3_access_key_file" ] && [ -z "$s3_secret_key_file" ]; } ||
    { [ -z "$s3_access_key_file" ] && [ -n "$s3_secret_key_file" ]; }; then
    echo "managed S3 access-key and secret-key files must both be set or both omitted" >&2
    exit 1
  fi
  for legacy_name in BRUNN_MINIO_ENDPOINT BRUNN_MINIO_REGION \
    BRUNN_MINIO_BUCKET BRUNN_MINIO_ACCESS_KEY \
    BRUNN_MINIO_SECRET_KEY; do
    [ -z "$(read_value "$legacy_name")" ] || {
      echo "$legacy_name must be empty in managed-s3 mode" >&2
      exit 1
    }
  done
  managed_backup_root=$(require_value BRUNN_MANAGED_BACKUP_ROOT)
  case "$managed_backup_root" in
    /*replace*|/*example*|/*placeholder*)
      echo "BRUNN_MANAGED_BACKUP_ROOT must name an approved durable destination" >&2
      exit 1
      ;;
    /*)
      ;;
    *)
      echo "BRUNN_MANAGED_BACKUP_ROOT must be an absolute durable path" >&2
      exit 1
      ;;
  esac
fi
for image_name in BRUNN_API_IMAGE BRUNN_WEB_IMAGE \
  BRUNN_MCP_IMAGE BRUNN_EDGE_IMAGE; do
  validate_release_image "$image_name"
done

secrets_dir=$(require_value BRUNN_SECRETS_DIR)
case "$secrets_dir" in
  /*)
    ;;
  *)
    env_parent=$(CDPATH= cd -- "$(dirname "$env_file")" && pwd)
    secrets_dir="$env_parent/$secrets_dir"
    ;;
esac
"$root/scripts/validate-production-secrets.sh" "$secrets_dir" "$object_store_mode"

echo "production configuration contract valid: host=$public_host revision=$release_revision object_store=$object_store_mode"

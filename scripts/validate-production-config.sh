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

environment=$(require_value STRAYLIGHT_ENV)
[ "$environment" = "production" ] || {
  echo "STRAYLIGHT_ENV must be production" >&2
  exit 1
}

release_revision=$(require_value STRAYLIGHT_RELEASE_REVISION)
case "$release_revision" in
  *[!0-9a-f]*)
    echo "STRAYLIGHT_RELEASE_REVISION must be a lowercase 40-character Git commit" >&2
    exit 1
    ;;
esac
[ "${#release_revision}" -eq 40 ] || {
  echo "STRAYLIGHT_RELEASE_REVISION must be a lowercase 40-character Git commit" >&2
  exit 1
}
[ "$release_revision" != "0000000000000000000000000000000000000000" ] || {
  echo "STRAYLIGHT_RELEASE_REVISION must not be the template revision" >&2
  exit 1
}

dd_version=$(require_value DD_VERSION)
[ "$dd_version" = "$release_revision" ] || {
  echo "DD_VERSION must equal STRAYLIGHT_RELEASE_REVISION" >&2
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

require_exact STRAYLIGHT_EMBEDDING_PROVIDER openai
require_exact STRAYLIGHT_ALLOW_DEGRADED_EMBEDDINGS false
require_exact STRAYLIGHT_EMBEDDING_MODEL text-embedding-3-small
require_exact STRAYLIGHT_EMBEDDING_DIMENSIONS 1536
require_exact STRAYLIGHT_CAPTURE_MODEL gpt-5.6
require_exact STRAYLIGHT_CAPTURE_MAX_OUTPUT_TOKENS 8192
require_exact STRAYLIGHT_DREAM_MODEL gpt-5.6
require_exact STRAYLIGHT_MATERIALIZE_TOKEN_BUDGET 24000
require_exact OPENAI_BASE_URL https://api.openai.com/v1
require_exact STRAYLIGHT_DREAM_SCHEDULER_ENABLED true
require_exact STRAYLIGHT_METRICS_ENABLED true
require_exact STRAYLIGHT_DOGSTATSD_ADDR datadog-agent:8125

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

require_uint_between STRAYLIGHT_REQUESTS_PER_MINUTE 60 6000 >/dev/null
require_uint_between STRAYLIGHT_REQUEST_TIMEOUT_SECONDS 5 120 >/dev/null
require_uint_between STRAYLIGHT_READINESS_TIMEOUT_SECONDS 1 15 >/dev/null
require_uint_between STRAYLIGHT_METRICS_FLUSH_SECONDS 1 30 >/dev/null
backup_retention=$(require_uint_between STRAYLIGHT_BACKUP_RETENTION_DAYS 1 90)
deletion_retention=$(
  require_uint_between STRAYLIGHT_ACCOUNT_DELETION_BACKUP_RETENTION_DAYS 1 90
)
[ "$backup_retention" -le "$deletion_retention" ] || {
  echo "STRAYLIGHT_BACKUP_RETENTION_DAYS must not exceed account-deletion backup retention" >&2
  exit 1
}

notify=$(require_value STRAYLIGHT_DATADOG_NOTIFY)
case "$notify" in
  *replace*|*example*|*@example.com|*@example.net|*@example.org)
    echo "STRAYLIGHT_DATADOG_NOTIFY must contain an approved non-placeholder destination" >&2
    exit 1
    ;;
esac

[ -z "$(read_value STRAYLIGHT_ALLOWED_ORIGINS)" ] || {
  echo "STRAYLIGHT_ALLOWED_ORIGINS must remain empty for the same-origin production SPA" >&2
  exit 1
}

public_host=$(require_value STRAYLIGHT_PUBLIC_HOST)
case "$public_host" in
  memory.example.com|*.example.com|*.example.net|*.example.org|*.invalid|*.test|localhost|*.localhost|*://*|*/*|*:*|*[!A-Za-z0-9.-]*)
    echo "STRAYLIGHT_PUBLIC_HOST must be an approved DNS hostname without a scheme, port, path, or placeholder domain" >&2
    exit 1
    ;;
esac
case "$public_host" in
  .*|*.|*..*)
    echo "STRAYLIGHT_PUBLIC_HOST is not a valid DNS hostname" >&2
    exit 1
    ;;
esac
case "$public_host" in
  *.*)
    ;;
  *)
    echo "STRAYLIGHT_PUBLIC_HOST must be a fully qualified DNS hostname" >&2
    exit 1
    ;;
esac

acme_email=$(require_value STRAYLIGHT_ACME_EMAIL)
case "$acme_email" in
  *@*.*)
    ;;
  *)
    echo "STRAYLIGHT_ACME_EMAIL must be a valid certificate-notification address" >&2
    exit 1
    ;;
esac
case "$acme_email" in
  *@example.com|*@example.net|*@example.org)
    echo "STRAYLIGHT_ACME_EMAIL must not use a placeholder domain" >&2
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
validate_digest_image STRAYLIGHT_DATABASE_IMAGE \
  "$(require_value STRAYLIGHT_DATABASE_IMAGE)"
validate_digest_image STRAYLIGHT_OBJECT_STORE_IMAGE \
  "$(require_value STRAYLIGHT_OBJECT_STORE_IMAGE)"
validate_digest_image STRAYLIGHT_OBJECT_STORE_CLIENT_IMAGE \
  "$(require_value STRAYLIGHT_OBJECT_STORE_CLIENT_IMAGE)"
for image_name in STRAYLIGHT_API_IMAGE STRAYLIGHT_WEB_IMAGE \
  STRAYLIGHT_MCP_IMAGE STRAYLIGHT_EDGE_IMAGE; do
  validate_release_image "$image_name"
done

secrets_dir=$(require_value STRAYLIGHT_SECRETS_DIR)
case "$secrets_dir" in
  /*)
    ;;
  *)
    env_parent=$(CDPATH= cd -- "$(dirname "$env_file")" && pwd)
    secrets_dir="$env_parent/$secrets_dir"
    ;;
esac
"$root/scripts/validate-production-secrets.sh" "$secrets_dir"

echo "production configuration contract valid: host=$public_host revision=$release_revision"

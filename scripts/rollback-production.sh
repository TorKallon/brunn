#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
env_file=${ENV_FILE:-"$root/production.env"}
project=${COMPOSE_PROJECT_NAME:-straylight}

[ -f "$env_file" ] || {
  echo "rollback environment file does not exist: $env_file" >&2
  exit 1
}
"$root/scripts/validate-production-config.sh" "$env_file"
"$root/scripts/verify-production-images.sh" "$env_file"

read_value() {
  awk -F= -v key="$1" '
    $1 == key {
      sub(/^[^=]*=/, "")
      print
      exit
    }
  ' "$env_file"
}

target_image=$(read_value STRAYLIGHT_API_IMAGE)
target_revision=$(read_value STRAYLIGHT_RELEASE_REVISION)
host=$(read_value STRAYLIGHT_PUBLIC_HOST)
object_store_mode=$(read_value STRAYLIGHT_OBJECT_STORE_MODE)
object_store_mode=${object_store_mode:-self-hosted-minio}
managed_overlay=
if [ "$object_store_mode" = "managed-s3" ]; then
  managed_overlay="$root/compose.managed-s3.yaml"
fi

ENV_FILE="$env_file" \
  COMPOSE_PROJECT_NAME="$project" \
  COMPOSE_OVERRIDE_FILE="$root/compose.production.yaml" \
  COMPOSE_MANAGED_S3_FILE="$managed_overlay" \
  "$root/scripts/verify-rollback-compatibility.sh" "$target_image"

compose() {
  if [ -n "$managed_overlay" ]; then
    docker compose \
      --project-name "$project" \
      --env-file "$env_file" \
      --file "$root/compose.yaml" \
      --file "$root/compose.production.yaml" \
      --file "$managed_overlay" \
      --profile observability \
      "$@"
  else
    docker compose \
      --project-name "$project" \
      --env-file "$env_file" \
      --file "$root/compose.yaml" \
      --file "$root/compose.production.yaml" \
      --profile observability \
      "$@"
  fi
}

compose up -d --no-build --pull never --no-deps api worker web edge

for service in api web edge; do
  attempts=0
  while [ "$attempts" -lt 120 ]; do
    container_id=$(compose ps -q "$service")
    state=$(
      docker inspect \
        --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' \
        "$container_id" 2>/dev/null || true
    )
    [ "$state" = "healthy" ] && break
    attempts=$((attempts + 1))
    sleep 1
  done
  [ "$attempts" -lt 120 ] || {
    echo "$service did not become healthy during rollback" >&2
    exit 1
  }
done

worker_id=$(compose ps -q worker)
[ "$(docker inspect --format '{{.State.Running}}' "$worker_id")" = "true" ] || {
  echo "worker is not running after rollback" >&2
  exit 1
}

"$root/scripts/check-public-health.sh" "$host"
echo "application rollback PASS: revision=$target_revision"

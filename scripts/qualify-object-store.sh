#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
env_file=${ENV_FILE:-"$root/.env"}
compose_override_file=${COMPOSE_OVERRIDE_FILE:-}
compose_managed_s3_file=${COMPOSE_MANAGED_S3_FILE:-}
project=${COMPOSE_PROJECT_NAME:-brunn}

[ -f "$env_file" ] || {
  echo "environment file does not exist: $env_file" >&2
  exit 1
}

if [ -n "$compose_managed_s3_file" ]; then
  [ -n "$compose_override_file" ] || {
    echo "COMPOSE_OVERRIDE_FILE is required with COMPOSE_MANAGED_S3_FILE" >&2
    exit 64
  }
  docker compose \
    --project-name "$project" \
    --env-file "$env_file" \
    --file "$root/compose.yaml" \
    --file "$compose_override_file" \
    --file "$compose_managed_s3_file" \
    run --rm -T --no-deps migrate object-store-check
elif [ -n "$compose_override_file" ]; then
  docker compose \
    --project-name "$project" \
    --env-file "$env_file" \
    --file "$root/compose.yaml" \
    --file "$compose_override_file" \
    run --rm -T --no-deps migrate object-store-check
else
  docker compose \
    --project-name "$project" \
    --env-file "$env_file" \
    --file "$root/compose.yaml" \
    run --rm -T --no-deps migrate object-store-check
fi

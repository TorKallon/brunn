#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
env_file=${ENV_FILE:-"$root/.env"}
compose_override_file=${COMPOSE_OVERRIDE_FILE:-}
project=${COMPOSE_PROJECT_NAME:-straylight}

[ -f "$env_file" ] || {
  echo "environment file does not exist: $env_file" >&2
  exit 1
}

if [ -n "$compose_override_file" ]; then
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

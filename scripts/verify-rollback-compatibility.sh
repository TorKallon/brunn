#!/bin/sh
set -eu
umask 077

usage() {
  echo "usage: $0 TARGET_API_IMAGE" >&2
  exit 64
}

[ "$#" -eq 1 ] || usage
target_image=$1
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
env_file=${ENV_FILE:-"$root/production.env"}
compose_override_file=${COMPOSE_OVERRIDE_FILE-"$root/compose.production.yaml"}
compose_managed_s3_file=${COMPOSE_MANAGED_S3_FILE:-}
project=${COMPOSE_PROJECT_NAME:-straylight}
probe_name="$project-rollback-probe-$$"
temp_dir=$(mktemp -d "${TMPDIR:-/tmp}/straylight-rollback-probe.XXXXXX")
override_file="$temp_dir/compose.rollback-probe.yaml"

[ -f "$env_file" ] || {
  echo "environment file does not exist: $env_file" >&2
  exit 1
}
docker image inspect "$target_image" >/dev/null

printf '%s\n' \
  'services:' \
  '  api:' \
  '    build: !reset null' \
  '    image: ${STRAYLIGHT_ROLLBACK_PROBE_IMAGE:?set probe image}' \
  >"$override_file"

compose() {
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
      --file "$override_file" \
      "$@"
  elif [ -n "$compose_override_file" ]; then
    docker compose \
      --project-name "$project" \
      --env-file "$env_file" \
      --file "$root/compose.yaml" \
      --file "$compose_override_file" \
      --file "$override_file" \
      "$@"
  else
    docker compose \
      --project-name "$project" \
      --env-file "$env_file" \
      --file "$root/compose.yaml" \
      --file "$override_file" \
      "$@"
  fi
}

cleanup() {
  status=$?
  trap - EXIT INT TERM
  docker rm --force "$probe_name" >/dev/null 2>&1 || true
  rm -rf "$temp_dir"
  exit "$status"
}
trap cleanup EXIT INT TERM

export STRAYLIGHT_ROLLBACK_PROBE_IMAGE=$target_image
compose run --detach --no-deps \
  --name "$probe_name" \
  --publish "127.0.0.1::8080" \
  api serve >/dev/null

attempts=0
host_port=
while [ "$attempts" -lt 60 ]; do
  host_port=$(docker port "$probe_name" 8080/tcp 2>/dev/null |
    awk -F: 'NR == 1 {print $NF}')
  if [ -n "$host_port" ] &&
    curl --fail --silent --show-error \
      --connect-timeout 2 \
      --max-time 5 \
      "http://127.0.0.1:$host_port/ready" \
      >"$temp_dir/ready.json" 2>/dev/null &&
    jq -e '
      .status == "ready"
      and ((.dependencies // {}) | all(.[]; . == "ready"))
    ' "$temp_dir/ready.json" >/dev/null; then
    break
  fi
  state=$(docker inspect --format '{{.State.Status}}' "$probe_name")
  [ "$state" = "running" ] || {
    docker logs --tail 100 "$probe_name" >&2
    echo "rollback candidate exited before becoming ready" >&2
    exit 1
  }
  attempts=$((attempts + 1))
  sleep 1
done
[ "$attempts" -lt 60 ] || {
  docker logs --tail 100 "$probe_name" >&2
  echo "rollback candidate did not become ready against the current schema" >&2
  exit 1
}

if [ -n "${PROBE_TOKEN_FILE:-}" ]; then
  [ -f "$PROBE_TOKEN_FILE" ] && [ ! -L "$PROBE_TOKEN_FILE" ] || {
    echo "PROBE_TOKEN_FILE must be a regular file" >&2
    exit 1
  }
  token=$(cat "$PROBE_TOKEN_FILE")
  curl --fail --silent --show-error \
    --connect-timeout 5 \
    --max-time 15 \
    --header "Authorization: Bearer $token" \
    "http://127.0.0.1:$host_port/v1/me" \
    >"$temp_dir/me.json"
  jq -e '.user_id != null and (.capabilities | length > 0)' \
    "$temp_dir/me.json" >/dev/null
fi

echo "rollback compatibility PASS: image=$target_image current_schema=ready"

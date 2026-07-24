#!/bin/sh
set -eu
umask 077

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
env_file=${ENV_FILE:-"$root/production.env"}
backup_root=${BACKUP_ROOT:-"$root/backups"}
record_root=${DEPLOYMENT_RECORD_ROOT:-"$root/deployment-records"}
project=${COMPOSE_PROJECT_NAME:-straylight}

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

revision=$(read_value STRAYLIGHT_RELEASE_REVISION)
host=$(read_value STRAYLIGHT_PUBLIC_HOST)
head_revision=$(git -C "$root" rev-parse HEAD)
[ "$head_revision" = "$revision" ] || {
  echo "production revision $revision does not match checked-out commit $head_revision" >&2
  exit 1
}
git -C "$root" diff --quiet
git -C "$root" diff --cached --quiet
[ -z "$(git -C "$root" ls-files --others --exclude-standard)" ] || {
  echo "production deployment requires a clean Git worktree" >&2
  exit 1
}

compose() {
  docker compose \
    --project-name "$project" \
    --env-file "$env_file" \
    --file "$root/compose.yaml" \
    --file "$root/compose.production.yaml" \
    --profile observability \
    "$@"
}

running=0
for service in db minio api worker; do
  container_id=$(compose ps -q "$service")
  if [ -n "$container_id" ] &&
    [ "$(docker inspect --format '{{.State.Running}}' "$container_id")" = "true" ]; then
    running=$((running + 1))
  fi
done

predeploy_backup=
if [ "$running" -eq 4 ]; then
  backup_log=$(mktemp "${TMPDIR:-/tmp}/straylight-predeploy-backup.XXXXXX")
  if ENV_FILE="$env_file" \
    COMPOSE_PROJECT_NAME="$project" \
    COMPOSE_OVERRIDE_FILE="$root/compose.production.yaml" \
    "$root/scripts/backup.sh" "$backup_root" >"$backup_log" 2>&1; then
    cat "$backup_log"
    predeploy_backup=$(sed -n 's/^coordinated backup complete: //p' "$backup_log")
    rm "$backup_log"
  else
    cat "$backup_log" >&2
    rm "$backup_log"
    exit 1
  fi
elif [ "$running" -ne 0 ]; then
  echo "refusing deployment over a partially running canonical stack" >&2
  exit 1
fi

wait_healthy() {
  service=$1
  attempts=0
  while [ "$attempts" -lt 180 ]; do
    container_id=$(compose ps -q "$service")
    state=$(
      docker inspect \
        --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' \
        "$container_id" 2>/dev/null || true
    )
    [ "$state" = "healthy" ] && return 0
    attempts=$((attempts + 1))
    sleep 1
  done
  echo "$service did not become healthy" >&2
  return 1
}

wait_completed() {
  service=$1
  attempts=0
  while [ "$attempts" -lt 180 ]; do
    container_id=$(compose ps -a -q "$service")
    if [ -n "$container_id" ]; then
      state=$(docker inspect --format '{{.State.Status}}' "$container_id")
      if [ "$state" = "exited" ]; then
        exit_code=$(docker inspect --format '{{.State.ExitCode}}' "$container_id")
        [ "$exit_code" = "0" ] || {
          compose logs --tail 200 "$service" >&2
          return 1
        }
        return 0
      fi
    fi
    attempts=$((attempts + 1))
    sleep 1
  done
  echo "$service did not complete" >&2
  return 1
}

deploy_candidate() {
  compose up -d --no-build --pull never db minio
  wait_healthy db
  wait_healthy minio
  compose up -d --no-build --pull never --force-recreate minio-init
  wait_completed minio-init
  ENV_FILE="$env_file" \
    COMPOSE_PROJECT_NAME="$project" \
    COMPOSE_OVERRIDE_FILE="$root/compose.production.yaml" \
    "$root/scripts/qualify-object-store.sh"
  compose up -d --no-build --pull never --force-recreate migrate
  wait_completed migrate
  compose up -d --no-build --pull never api worker web datadog-agent edge
  wait_healthy api
  wait_healthy web
  wait_healthy datadog-agent
  wait_healthy edge
  worker_id=$(compose ps -q worker)
  [ "$(docker inspect --format '{{.State.Running}}' "$worker_id")" = "true" ]
  "$root/scripts/check-public-health.sh" "$host"
}

if ! deploy_candidate; then
  echo "candidate failed its deployment health gate" >&2
  if [ -n "${ROLLBACK_ENV_FILE:-}" ]; then
    echo "attempting explicitly configured application rollback" >&2
    ENV_FILE="$ROLLBACK_ENV_FILE" \
      COMPOSE_PROJECT_NAME="$project" \
      "$root/scripts/rollback-production.sh"
  fi
  exit 1
fi

mkdir -p "$record_root"
chmod 0700 "$record_root"
stamp=$(date -u +%Y%m%dT%H%M%SZ)
record_dir="$record_root/$stamp-$revision"
mkdir -m 0700 "$record_dir"
compose config --hash '*' >"$record_dir/compose-service-hashes.txt"
compose config --images >"$record_dir/configured-images.txt"
jq -n \
  --arg format "straylight-deployment@v1" \
  --arg deployed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg revision "$revision" \
  --arg host "$host" \
  --arg predeploy_backup "$predeploy_backup" \
  '{
    format: $format,
    deployed_at: $deployed_at,
    revision: $revision,
    public_host: $host,
    predeploy_backup: (
      if $predeploy_backup == "" then null else $predeploy_backup end
    ),
    compose_service_hashes: "compose-service-hashes.txt",
    configured_images: "configured-images.txt"
  }' >"$record_dir/manifest.json"
(
  cd "$record_dir"
  shasum -a 256 compose-service-hashes.txt configured-images.txt manifest.json \
    >CHECKSUMS.sha256
)

echo "production deployment PASS: revision=$revision record=$record_dir"

#!/bin/sh
set -eu
umask 077

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
env_file=${ENV_FILE:-"$root/production.env"}
backup_root_override=${BACKUP_ROOT:-}
record_root=${DEPLOYMENT_RECORD_ROOT:-"$root/deployment-records"}
project=${COMPOSE_PROJECT_NAME:-brunn}

. "$root/scripts/deploy-steps.sh"

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

revision=$(read_value BRUNN_RELEASE_REVISION)
host=$(read_value BRUNN_PUBLIC_HOST)
object_store_mode=$(read_value BRUNN_OBJECT_STORE_MODE)
object_store_mode=${object_store_mode:-self-hosted-minio}
managed_overlay=
case "$object_store_mode" in
  self-hosted-minio)
    backup_root=${backup_root_override:-"$root/backups"}
    ;;
  managed-s3)
    managed_overlay="$root/compose.managed-s3.yaml"
    backup_root=${backup_root_override:-$(read_value BRUNN_MANAGED_BACKUP_ROOT)}
    ;;
  *)
    echo "unsupported object-store mode: $object_store_mode" >&2
    exit 1
    ;;
esac
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

running=0
expected_running=0
running_services="db api worker"
[ "$object_store_mode" = "managed-s3" ] ||
  running_services="db minio api worker"
for service in $running_services; do
  expected_running=$((expected_running + 1))
  container_id=$(compose ps -q "$service")
  if [ -n "$container_id" ] &&
    [ "$(docker inspect --format '{{.State.Running}}' "$container_id")" = "true" ]; then
    running=$((running + 1))
  fi
done

predeploy_backup=
if [ "$running" -eq "$expected_running" ]; then
  backup_log=$(mktemp "${TMPDIR:-/tmp}/brunn-predeploy-backup.XXXXXX")
  if [ "$object_store_mode" = "managed-s3" ]; then
    backup_command="$root/scripts/managed-s3-backup.sh"
  else
    backup_command="$root/scripts/backup.sh"
  fi
  if ENV_FILE="$env_file" \
    COMPOSE_PROJECT_NAME="$project" \
    COMPOSE_OVERRIDE_FILE="$root/compose.production.yaml" \
    COMPOSE_MANAGED_S3_FILE="$managed_overlay" \
    LEAVE_WRITERS_STOPPED=true \
    "$backup_command" "$backup_root" >"$backup_log" 2>&1; then
    cat "$backup_log"
    predeploy_backup=$(sed -n \
      -e 's/^coordinated backup complete: //p' \
      -e 's/^managed S3 coordinated backup complete: //p' \
      "$backup_log")
    rm "$backup_log"
  else
    cat "$backup_log" >&2
    rm "$backup_log"
    exit 1
  fi
elif [ "$running" -ne 0 ]; then
  echo "refusing deployment over a partially running canonical stack" >&2
  exit 1
else
  existing_db_container=$(compose ps -a -q db)
  existing_db_volume=$(docker volume ls -q \
    --filter "label=com.docker.compose.project=$project" \
    --filter "label=com.docker.compose.volume=postgres_data" | head -n 1)
  if [ -n "$existing_db_container" ] || [ -n "$existing_db_volume" ]; then
    echo "refusing deployment over a stopped database without a verified pre-deploy backup" >&2
    exit 1
  fi
  [ "${INITIAL_DEPLOY:-false}" = "true" ] || {
    echo "an empty installation requires explicit INITIAL_DEPLOY=true" >&2
    exit 1
  }
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

qualify_candidate_object_store() {
  ENV_FILE="$env_file" \
    COMPOSE_PROJECT_NAME="$project" \
    COMPOSE_OVERRIDE_FILE="$root/compose.production.yaml" \
    COMPOSE_MANAGED_S3_FILE="$managed_overlay" \
    "$root/scripts/qualify-object-store.sh"
}

verify_candidate_worker() {
  worker_id=$(compose ps -q worker) || return $?
  [ -n "$worker_id" ] || {
    echo "worker has no candidate container" >&2
    return 1
  }
  worker_state=$(docker inspect --format '{{.State.Running}}' "$worker_id") ||
    return $?
  [ "$worker_state" = "true" ] || {
    echo "worker candidate is not running" >&2
    return 1
  }
}

deploy_candidate() {
  deploy_step database-start \
    compose up -d --no-build --pull never db || return $?
  deploy_step database-ready wait_healthy db || return $?
  if [ "$object_store_mode" = "self-hosted-minio" ]; then
    deploy_step object-store-start \
      compose up -d --no-build --pull never minio || return $?
    deploy_step object-store-ready wait_healthy minio || return $?
    deploy_step object-store-init-start \
      compose up -d --no-build --pull never --force-recreate minio-init ||
      return $?
    deploy_step object-store-init-complete wait_completed minio-init ||
      return $?
  fi
  deploy_step object-store-qualify qualify_candidate_object_store || return $?
  # Migrations may strengthen write-path invariants. Keep every old writer
  # quiescent from the first schema change until the candidate is running.
  deploy_step writers-stop compose stop api worker || return $?
  deploy_step migration-start \
    compose up -d --no-build --pull never --force-recreate migrate || return $?
  deploy_step migration-complete wait_completed migrate || return $?
  deploy_step candidate-start \
    compose up -d --no-build --pull never api worker web datadog-agent edge ||
    return $?
  deploy_step api-ready wait_healthy api || return $?
  deploy_step web-ready wait_healthy web || return $?
  deploy_step datadog-ready wait_healthy datadog-agent || return $?
  deploy_step edge-ready wait_healthy edge || return $?
  deploy_step worker-ready verify_candidate_worker || return $?
  deploy_step public-health \
    "$root/scripts/check-public-health.sh" "$host" || return $?
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
  --arg format "brunn-deployment@v1" \
  --arg deployed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg revision "$revision" \
  --arg host "$host" \
  --arg object_store_mode "$object_store_mode" \
  --arg predeploy_backup "$predeploy_backup" \
  '{
    format: $format,
    deployed_at: $deployed_at,
    revision: $revision,
    public_host: $host,
    object_store_mode: $object_store_mode,
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

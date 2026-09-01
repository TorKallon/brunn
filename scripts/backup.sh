#!/bin/sh
set -eu
umask 077

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
env_file=${ENV_FILE:-"$root/.env"}
backup_root=${1:-"$root/backups"}
compose_file=${COMPOSE_FILE:-"$root/compose.yaml"}
compose_override_file=${COMPOSE_OVERRIDE_FILE:-}
project=${COMPOSE_PROJECT_NAME:-brunn}
retention_days=${BRUNN_BACKUP_RETENTION_DAYS:-30}

case "$retention_days" in
  ''|*[!0-9]*)
    echo "BRUNN_BACKUP_RETENTION_DAYS must be a positive integer" >&2
    exit 64
    ;;
esac
[ "$retention_days" -ge 1 ] && [ "$retention_days" -le 90 ] || {
  echo "BRUNN_BACKUP_RETENTION_DAYS must be between 1 and 90" >&2
  exit 64
}

[ -f "$env_file" ] || {
  echo "missing environment file: $env_file" >&2
  exit 1
}

compose() {
  if [ -n "$compose_override_file" ]; then
    docker compose \
      --project-name "$project" \
      --env-file "$env_file" \
      --file "$compose_file" \
      --file "$compose_override_file" \
      "$@"
  else
    docker compose \
      --project-name "$project" \
      --env-file "$env_file" \
      --file "$compose_file" \
      "$@"
  fi
}

container_running() {
  container_id=$(compose ps -q "$1")
  [ -n "$container_id" ] &&
    [ "$(docker inspect --format '{{.State.Running}}' "$container_id")" = "true" ]
}

container_env() {
  docker inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$1" |
    awk -F= -v key="$2" '
      $1 == key {
        sub(/^[^=]*=/, "")
        print
        exit
      }
    '
}

backup_id=$(date -u +%Y%m%dT%H%M%SZ)-$(uuidgen | tr '[:upper:]' '[:lower:]')
leave_services_stopped=${LEAVE_WRITERS_STOPPED:-false}
case "$leave_services_stopped" in
  true|false)
    ;;
  *)
    echo "LEAVE_WRITERS_STOPPED must be true or false" >&2
    exit 64
    ;;
esac

mkdir -p "$backup_root"
backup_root=$(CDPATH= cd -- "$backup_root" && pwd)
operation_lock_dir="$backup_root/.recovery-operation.lock"
work_dir="$backup_root/.$backup_id.partial"
final_dir="$backup_root/$backup_id"
operation_lock_acquired=false
work_dir_created=false
writers_stop_attempted=false
minio_stop_attempted=false
completed=false
db_container=
api_container=
worker_container=
minio_container=

wait_original_container_ready() {
  container_id=$1
  service=$2
  expected=$3
  attempts=0
  state=
  while [ "$attempts" -lt 90 ]; do
    state=$(docker inspect \
      --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' \
      "$container_id" 2>/dev/null || true)
    if [ "$state" = "$expected" ]; then
      return 0
    fi
    attempts=$((attempts + 1))
    sleep 1
  done
  echo "$service did not return to $expected after backup recovery" >&2
  return 1
}

restart_original_services() {
  docker start "$minio_container" >/dev/null || return $?
  wait_original_container_ready "$minio_container" minio healthy || return $?
  docker start "$api_container" >/dev/null || return $?
  docker start "$worker_container" >/dev/null || return $?
  wait_original_container_ready "$api_container" api healthy || return $?
  wait_original_container_ready "$worker_container" worker running || return $?
}

recover_services() {
  status=$?
  recovery_failed=false
  trap - EXIT INT TERM
  if [ "$minio_stop_attempted" = true ] ||
    [ "$writers_stop_attempted" = true ]; then
    if [ "$completed" != true ] || [ "$leave_services_stopped" != true ]; then
      if ! restart_original_services; then
        echo "backup recovery could not restore the original services to readiness" >&2
        recovery_failed=true
      fi
    fi
  fi
  if [ "$completed" != true ] && [ "$work_dir_created" = true ]; then
    rm -rf "$work_dir"
  fi
  if [ "$operation_lock_acquired" = true ]; then
    rmdir "$operation_lock_dir" >/dev/null 2>&1 || true
  fi
  if [ "$recovery_failed" = true ]; then
    exit 1
  fi
  exit "$status"
}
trap recover_services EXIT INT TERM

mkdir "$operation_lock_dir" 2>/dev/null || {
  echo "another backup, restore, or prune operation holds $operation_lock_dir" >&2
  exit 1
}
operation_lock_acquired=true
mkdir "$work_dir"
work_dir_created=true

for service in db minio api worker; do
  container_running "$service" || {
    echo "$service must be running before a coordinated backup" >&2
    exit 1
  }
done

db_container=$(compose ps -q db)
api_container=$(compose ps -q api)
worker_container=$(compose ps -q worker)
minio_container=$(compose ps -q minio)

db_user=$(container_env "$db_container" POSTGRES_USER)
db_name=$(container_env "$db_container" POSTGRES_DB)
bucket=$(container_env "$api_container" BRUNN_MINIO_BUCKET)
for value_name in db_user db_name bucket; do
  eval "value=\${$value_name}"
  [ -n "$value" ] || {
    echo "could not resolve $value_name from the running stack" >&2
    exit 1
  }
done

expires_at=$(docker exec "$db_container" psql \
  --username "$db_user" \
  --dbname "$db_name" \
  --tuples-only \
  --no-align \
  --no-psqlrc \
  --command "SELECT to_char(clock_timestamp()+interval '$retention_days days','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')" |
  tr -d '[:space:]')

started_epoch=$(date +%s)
created_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
echo "quiescing API and worker"
writers_stop_attempted=true
compose stop --timeout 30 api worker >/dev/null

echo "bringing the quiesced database to the current schema"
compose run --rm migrate >/dev/null

active_deletions=$(docker exec "$db_container" psql \
  --username "$db_user" \
  --dbname "$db_name" \
  --tuples-only \
  --no-align \
  --no-psqlrc \
  --command "SELECT count(*) FROM brunn.account_deletion_requests WHERE status IN ('queued','running')" |
  tr -d '[:space:]')
[ "$active_deletions" = "0" ] || {
  echo "cannot snapshot while $active_deletions account deletion request(s) are mutating canonical data" >&2
  exit 1
}
echo "pinning legacy database references to exact object versions"
compose run --rm -T migrate object-store-backup pin-database \
  >"$work_dir/database-object-pinning.json"
jq -e '
  (.asset_versions_pinned | type == "number" and . >= 0)
  and (.upload_canonical_versions_pinned | type == "number" and . >= 0)
  and (.account_export_versions_pinned | type == "number" and . >= 0)
  and (.objects_stream_verified | type == "number" and . >= 0)
' "$work_dir/database-object-pinning.json" >/dev/null

echo "verifying database references against exact object bytes"
compose run --rm -T migrate object-store-backup verify-database \
  >"$work_dir/database-object-verification.json"
jq -e '
  (.references_verified | type == "number" and . >= 0)
  and (.unique_object_versions_verified | type == "number" and . >= 0)
  and (.logical_bytes_verified | type == "number" and . >= 0)
' "$work_dir/database-object-verification.json" >/dev/null

minio_volume=$(docker inspect --format \
  '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Name}}{{end}}{{end}}' \
  "$minio_container")
[ -n "$minio_volume" ] || {
  echo "could not identify the MinIO data volume" >&2
  exit 1
}

echo "capturing PostgreSQL snapshot"
docker exec -i "$db_container" pg_dump \
  --username "$db_user" \
  --dbname "$db_name" \
  --format custom \
  --compress zstd:9 \
  --serializable-deferrable \
  >"$work_dir/postgres.dump"

docker exec -i "$db_container" psql \
  --username "$db_user" \
  --dbname "$db_name" \
  --quiet \
  --no-psqlrc \
  <"$root/scripts/db-inventory.sql" \
  >"$work_dir/db-inventory.txt"
docker exec -i "$db_container" psql \
  --username "$db_user" \
  --dbname "$db_name" \
  --quiet \
  --no-psqlrc \
  <"$root/scripts/database-invariants.sql" |
  jq -S . >"$work_dir/database-invariants.json"
jq -e '.safe == true' "$work_dir/database-invariants.json" >/dev/null || {
  echo "database storage invariants are unsafe; refusing to certify backup" >&2
  exit 1
}

echo "capturing MinIO version inventory"
compose run --rm -T --no-deps --entrypoint /bin/sh minio-init -ec '
  load_secret() {
    variable=$1
    file_variable="${variable}_FILE"
    eval "value=\${$variable:-}"
    eval "file=\${$file_variable:-}"
    if [ -n "$file" ]; then
      value=$(cat "$file")
      export "$variable=$value"
    fi
    [ -n "$value" ] || {
      echo "$variable is required" >&2
      exit 1
    }
  }
  load_secret MINIO_ROOT_USER
  load_secret MINIO_ROOT_PASSWORD
  mc alias set local http://minio:9000 \
    "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" >/dev/null
  mc ls --versions --recursive --json \
    "local/$BRUNN_MINIO_BUCKET"
' | LC_ALL=C sort >"$work_dir/minio-versions.jsonl"

echo "capturing immutable runtime identity"
compose config --hash '*' >"$work_dir/compose-service-hashes.txt"
runtime_inspect="$work_dir/.runtime-inspect.jsonl"
: >"$runtime_inspect"
for container_id in $(compose ps -a -q); do
  docker inspect "$container_id" |
    jq -c '
      .[0] |
      {
        service: .Config.Labels["com.docker.compose.service"],
        container_name: .Name[1:],
        configured_image: .Config.Image,
        immutable_image_id: .Image,
        state: .State.Status
      }
    ' >>"$runtime_inspect"
done
jq -s 'sort_by(.service, .container_name)' \
  "$runtime_inspect" >"$work_dir/runtime-images.json"
rm "$runtime_inspect"

echo "capturing stopped MinIO volume with all versions"
minio_stop_attempted=true
compose stop --timeout 30 minio >/dev/null
docker run --rm --network none \
  --volume "$minio_volume:/source:ro" \
  --volume "$work_dir:/backup" \
  alpine:3.23@sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40 \
  tar --numeric-owner --directory /source --create \
    --file /backup/minio-data.tar .

completed_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
finished_epoch=$(date +%s)
downtime_seconds=$((finished_epoch - started_epoch))
git_revision=$(git -C "$root" rev-parse HEAD 2>/dev/null || printf unknown)
git_dirty=false
if [ -n "$(git -C "$root" status --short 2>/dev/null || true)" ]; then
  git_dirty=true
fi

jq -n \
  --arg format "brunn-coordinated-backup@v2" \
  --arg backup_id "$backup_id" \
  --arg created_at "$created_at" \
  --arg completed_at "$completed_at" \
  --arg expires_at "$expires_at" \
  --arg project "$project" \
  --arg database "$db_name" \
  --arg bucket "$bucket" \
  --arg git_revision "$git_revision" \
  --argjson git_dirty "$git_dirty" \
  --argjson quiesced_seconds "$downtime_seconds" \
  --argjson retention_days "$retention_days" \
  '{
    format: $format,
    backup_id: $backup_id,
    created_at: $created_at,
    completed_at: $completed_at,
    expires_at: $expires_at,
    retention_days: $retention_days,
    source_project: $project,
    database: $database,
    object_bucket: $bucket,
    git_revision: $git_revision,
    git_dirty: $git_dirty,
    quiesced_seconds: $quiesced_seconds,
    consistency: {
      writers_stopped: true,
      postgres_snapshot: "serializable-deferrable",
      minio_snapshot: "stopped-volume-all-versions"
    },
    runtime_identity: {
      images: "runtime-images.json",
      compose_service_hashes: "compose-service-hashes.txt",
      database_invariants: "database-invariants.json",
      database_object_pinning: "database-object-pinning.json",
      database_object_verification: "database-object-verification.json"
    }
  }' >"$work_dir/manifest.json"

(
  cd "$work_dir"
  shasum -a 256 postgres.dump minio-data.tar \
    db-inventory.txt database-invariants.json database-object-pinning.json \
    database-object-verification.json minio-versions.jsonl runtime-images.json \
    compose-service-hashes.txt manifest.json >CHECKSUMS.sha256
)

"$root/scripts/verify-backup.sh" "$work_dir" >/dev/null
mv "$work_dir" "$final_dir"
work_dir_created=false
completed=true

if [ "$leave_services_stopped" = true ]; then
  echo "leaving the original MinIO, API, and worker stopped for the deployment gate"
else
  echo "restarting the exact MinIO, API, and worker containers stopped for this backup"
  restart_original_services
  minio_stop_attempted=false
  writers_stop_attempted=false
fi

"$root/scripts/verify-backup.sh" "$final_dir"
echo "coordinated backup complete: $final_dir"

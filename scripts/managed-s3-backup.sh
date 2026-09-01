#!/bin/sh
set -eu
umask 077

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
env_file=${ENV_FILE:-"$root/production.env"}
backup_root_arg=${1:-}
project=${COMPOSE_PROJECT_NAME:-brunn}
production_overlay=${COMPOSE_OVERRIDE_FILE:-"$root/compose.production.yaml"}
managed_overlay=${COMPOSE_MANAGED_S3_FILE:-"$root/compose.managed-s3.yaml"}

[ -f "$env_file" ] || {
  echo "missing production environment file: $env_file" >&2
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
backup_root=$backup_root_arg
[ -n "$backup_root" ] ||
  backup_root=${BRUNN_MANAGED_BACKUP_ROOT:-}
[ -n "$backup_root" ] ||
  backup_root=$(read_value BRUNN_MANAGED_BACKUP_ROOT)
retention_days=${BRUNN_BACKUP_RETENTION_DAYS:-}
[ -n "$retention_days" ] ||
  retention_days=$(read_value BRUNN_BACKUP_RETENTION_DAYS)
retention_days=${retention_days:-30}

case "$backup_root" in
  /*)
    ;;
  *)
    echo "managed S3 backup requires an explicit absolute durable backup root" >&2
    exit 64
    ;;
esac
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
compose() {
  docker compose \
    --project-name "$project" \
    --env-file "$env_file" \
    --file "$root/compose.yaml" \
    --file "$production_overlay" \
    --file "$managed_overlay" \
    "$@"
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
leave_writers_stopped=${LEAVE_WRITERS_STOPPED:-false}
case "$leave_writers_stopped" in
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
completed=false
db_container=
api_container=
worker_container=

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
  echo "$service did not return to $expected after managed backup recovery" >&2
  return 1
}

restart_original_writers() {
  docker start "$api_container" >/dev/null || return $?
  docker start "$worker_container" >/dev/null || return $?
  wait_original_container_ready "$api_container" api healthy || return $?
  wait_original_container_ready "$worker_container" worker running || return $?
}

recover_services() {
  status=$?
  recovery_failed=false
  trap - EXIT INT TERM
  if [ "$writers_stop_attempted" = true ]; then
    if [ "$completed" != true ] || [ "$leave_writers_stopped" != true ]; then
      if ! restart_original_writers; then
        echo "managed backup recovery could not restore the original writers to readiness" >&2
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

for service in db api worker; do
  container_running "$service" || {
    echo "$service must be running before a managed S3 coordinated backup" >&2
    exit 1
  }
done

db_container=$(compose ps -q db)
api_container=$(compose ps -q api)
worker_container=$(compose ps -q worker)
db_user=$(container_env "$db_container" POSTGRES_USER)
db_name=$(container_env "$db_container" POSTGRES_DB)
bucket=$(container_env "$api_container" BRUNN_S3_BUCKET)
for value_name in db_user db_name bucket; do
  eval "value=\${$value_name}"
  [ -n "$value" ] || {
    echo "could not resolve $value_name from the running managed stack" >&2
    exit 1
  }
done

ENV_FILE="$env_file" \
  COMPOSE_PROJECT_NAME="$project" \
  COMPOSE_OVERRIDE_FILE="$production_overlay" \
  COMPOSE_MANAGED_S3_FILE="$managed_overlay" \
  "$root/scripts/qualify-object-store.sh" >/dev/null

expires_at=$(docker exec "$db_container" psql \
  --username "$db_user" --dbname "$db_name" --tuples-only --no-align \
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
  --username "$db_user" --dbname "$db_name" --tuples-only --no-align \
  --no-psqlrc \
  --command "SELECT count(*) FROM brunn.account_deletion_requests WHERE status IN ('queued','running')" |
  tr -d '[:space:]')
[ "$active_deletions" = "0" ] || {
  echo "cannot back up while $active_deletions account deletion request(s) are active" >&2
  exit 1
}
active_uploads=$(docker exec "$db_container" psql \
  --username "$db_user" --dbname "$db_name" --tuples-only --no-align \
  --no-psqlrc \
  --command "SELECT count(*) FROM brunn.asset_uploads WHERE status IN ('uploading','verifying')" |
  tr -d '[:space:]')
[ "$active_uploads" = "0" ] || {
  echo "cannot back up while $active_uploads resumable upload(s) are incomplete" >&2
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

echo "capturing PostgreSQL snapshot"
docker exec -i "$db_container" pg_dump \
  --username "$db_user" --dbname "$db_name" --format custom \
  --compress zstd:9 --serializable-deferrable >"$work_dir/postgres.dump"
docker exec -i "$db_container" psql \
  --username "$db_user" --dbname "$db_name" --quiet --no-psqlrc \
  <"$root/scripts/db-inventory.sql" >"$work_dir/db-inventory.txt"
docker exec -i "$db_container" psql \
  --username "$db_user" --dbname "$db_name" --quiet --no-psqlrc \
  <"$root/scripts/database-invariants.sql" |
  jq -S . >"$work_dir/database-invariants.json"
jq -e '.safe == true' "$work_dir/database-invariants.json" >/dev/null || {
  echo "database storage invariants are unsafe; refusing to certify backup" >&2
  exit 1
}
docker exec -i "$db_container" psql \
  --username "$db_user" --dbname "$db_name" --quiet --no-psqlrc \
  --command "SET brunn.backup_object_bucket TO '$bucket';" \
  --file - <"$root/scripts/database-object-references.sql" |
  jq -S . >"$work_dir/database-object-references.json"

echo "capturing every managed object version and delete marker"
compose run --rm -T --no-deps \
  --user "$(id -u):$(id -g)" \
  --volume "$work_dir:/backup" \
  migrate object-store-backup export --output /backup/object-store \
  >"$work_dir/object-store-export.json"

compose config --hash '*' >"$work_dir/compose-service-hashes.txt"
runtime_inspect="$work_dir/.runtime-inspect.jsonl"
: >"$runtime_inspect"
for container_id in $(compose ps -a -q); do
  docker inspect "$container_id" |
    jq -c '
      .[0] | {
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

completed_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
finished_epoch=$(date +%s)
git_revision=$(git -C "$root" rev-parse HEAD 2>/dev/null || printf unknown)
git_dirty=false
[ -z "$(git -C "$root" status --short 2>/dev/null || true)" ] || git_dirty=true
jq -n \
  --arg backup_id "$backup_id" \
  --arg created_at "$created_at" \
  --arg completed_at "$completed_at" \
  --arg expires_at "$expires_at" \
  --arg project "$project" \
  --arg database "$db_name" \
  --arg bucket "$bucket" \
  --arg git_revision "$git_revision" \
  --argjson git_dirty "$git_dirty" \
  --argjson retention_days "$retention_days" \
  --argjson quiesced_seconds "$((finished_epoch - started_epoch))" \
  '{
    format: "brunn-managed-s3-coordinated-backup@v1",
    backup_id: $backup_id,
    created_at: $created_at,
    completed_at: $completed_at,
    expires_at: $expires_at,
    retention_days: $retention_days,
    source_project: $project,
    database: $database,
    object_bucket: $bucket,
    object_archive: "object-store/manifest.json",
    git_revision: $git_revision,
    git_dirty: $git_dirty,
    quiesced_seconds: $quiesced_seconds,
    consistency: {
      writers_stopped: true,
      postgres_snapshot: "serializable-deferrable",
      object_snapshot: "portable-all-versions"
    },
    runtime_identity: {
      images: "runtime-images.json",
      compose_service_hashes: "compose-service-hashes.txt",
      database_invariants: "database-invariants.json",
      database_object_references: "database-object-references.json",
      database_object_pinning: "database-object-pinning.json",
      database_object_verification: "database-object-verification.json"
    }
  }' >"$work_dir/manifest.json"

(
  cd "$work_dir"
  shasum -a 256 postgres.dump db-inventory.txt database-invariants.json \
    database-object-references.json database-object-pinning.json \
    database-object-verification.json \
    object-store/manifest.json object-store-export.json runtime-images.json \
    compose-service-hashes.txt manifest.json >CHECKSUMS.sha256
)
"$root/scripts/verify-managed-backup.sh" "$work_dir" >/dev/null
mv "$work_dir" "$final_dir"
work_dir_created=false
completed=true

if [ "$leave_writers_stopped" = true ]; then
  echo "leaving the original API and worker stopped for the deployment gate"
else
  echo "restarting the exact API and worker containers stopped for this backup"
  restart_original_writers
  writers_stop_attempted=false
fi
"$root/scripts/verify-managed-backup.sh" "$final_dir"
echo "managed S3 coordinated backup complete: $final_dir"

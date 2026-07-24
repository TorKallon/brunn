#!/bin/sh
set -eu
umask 077

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
env_file=${ENV_FILE:-"$root/.env"}
backup_root=${1:-"$root/backups"}
compose_file=${COMPOSE_FILE:-"$root/compose.yaml"}
compose_override_file=${COMPOSE_OVERRIDE_FILE:-}
project=${COMPOSE_PROJECT_NAME:-straylight}
retention_days=${STRAYLIGHT_BACKUP_RETENTION_DAYS:-30}

case "$retention_days" in
  ''|*[!0-9]*)
    echo "STRAYLIGHT_BACKUP_RETENTION_DAYS must be a positive integer" >&2
    exit 64
    ;;
esac
[ "$retention_days" -ge 1 ] && [ "$retention_days" -le 90 ] || {
  echo "STRAYLIGHT_BACKUP_RETENTION_DAYS must be between 1 and 90" >&2
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

for service in db minio api worker; do
  container_running "$service" || {
    echo "$service must be running before a coordinated backup" >&2
    exit 1
  }
done

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
mkdir -p "$backup_root"
lock_dir="$backup_root/.backup.lock"
mkdir "$lock_dir" 2>/dev/null || {
  echo "another coordinated backup appears to be running: $lock_dir" >&2
  exit 1
}
work_dir="$backup_root/.$backup_id.partial"
final_dir="$backup_root/$backup_id"
mkdir "$work_dir"

writers_stopped=false
minio_stopped=false
completed=false

recover_services() {
  status=$?
  trap - EXIT INT TERM
  if [ "$minio_stopped" = true ]; then
    compose up -d minio >/dev/null 2>&1 || true
  fi
  if [ "$writers_stopped" = true ]; then
    compose up -d minio-init api worker >/dev/null 2>&1 || true
  fi
  if [ "$completed" != true ]; then
    rm -rf "$work_dir"
  fi
  rmdir "$lock_dir" >/dev/null 2>&1 || true
  exit "$status"
}
trap recover_services EXIT INT TERM

db_container=$(compose ps -q db)
api_container=$(compose ps -q api)
minio_container=$(compose ps -q minio)
db_user=$(container_env "$db_container" POSTGRES_USER)
db_name=$(container_env "$db_container" POSTGRES_DB)
bucket=$(container_env "$api_container" STRAYLIGHT_MINIO_BUCKET)
for value_name in db_user db_name bucket; do
  eval "value=\${$value_name}"
  [ -n "$value" ] || {
    echo "could not resolve $value_name from the running stack" >&2
    exit 1
  }
done

active_deletions=$(docker exec "$db_container" psql \
  --username "$db_user" \
  --dbname "$db_name" \
  --tuples-only \
  --no-align \
  --no-psqlrc \
  --command "SELECT count(*) FROM straylight.account_deletion_requests WHERE status IN ('queued','running')" |
  tr -d '[:space:]')
[ "$active_deletions" = "0" ] || {
  echo "cannot snapshot while $active_deletions account deletion request(s) are mutating canonical data" >&2
  exit 1
}

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
compose stop --timeout 30 api worker >/dev/null
writers_stopped=true

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
    "local/$STRAYLIGHT_MINIO_BUCKET"
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
compose stop --timeout 30 minio >/dev/null
minio_stopped=true
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
  --arg format "straylight-coordinated-backup@v2" \
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
      database_invariants: "database-invariants.json"
    }
  }' >"$work_dir/manifest.json"

(
  cd "$work_dir"
  shasum -a 256 postgres.dump minio-data.tar \
    db-inventory.txt database-invariants.json minio-versions.jsonl runtime-images.json \
    compose-service-hashes.txt manifest.json >CHECKSUMS.sha256
)

mv "$work_dir" "$final_dir"
completed=true

echo "restarting MinIO, API, and worker"
compose up -d minio minio-init api worker >/dev/null
minio_stopped=false
writers_stopped=false

"$root/scripts/verify-backup.sh" "$final_dir"
echo "coordinated backup complete: $final_dir"

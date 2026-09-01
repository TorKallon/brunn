#!/bin/sh
set -eu
umask 077

usage() {
  echo "usage: $0 BACKUP_DIR DRILL_ENV_FILE" >&2
  exit 64
}

[ "$#" -eq 2 ] || usage
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
backup_dir=$(CDPATH= cd -- "$1" && pwd)
backup_root=$(dirname "$backup_dir")
env_file=$(CDPATH= cd -- "$(dirname "$2")" && pwd)/$(basename "$2")
project=
production_overlay=${COMPOSE_OVERRIDE_FILE:-"$root/compose.production.yaml"}
managed_overlay=${COMPOSE_MANAGED_S3_FILE:-"$root/compose.managed-s3.yaml"}
restore_overlay="$root/compose.restore-drill.yaml"
started_epoch=$(date +%s)
temp_dir=$(mktemp -d "${TMPDIR:-/tmp}/brunn-managed-restore.XXXXXX")
host_baseline="$temp_dir/host-containers.tsv"
mapping=
state_dir=
operation_lock_dir="$backup_root/.recovery-operation.lock"
operation_lock_acquired=false
object_reference_integrity_verified=false

[ -f "$env_file" ] || {
  echo "restore drill environment file does not exist: $env_file" >&2
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
[ "$(read_value BRUNN_OBJECT_STORE_MODE)" = "managed-s3" ] || {
  echo "restore drill environment must set BRUNN_OBJECT_STORE_MODE=managed-s3" >&2
  exit 1
}
[ "$(read_value BRUNN_RESTORE_DRILL)" = "true" ] || {
  echo "restore drill environment must set BRUNN_RESTORE_DRILL=true" >&2
  exit 1
}
source_bucket=$(jq -r '.object_bucket' "$backup_dir/manifest.json")
backup_id=$(jq -r '.backup_id' "$backup_dir/manifest.json")
target_bucket=$(read_value BRUNN_S3_BUCKET)
[ -n "$target_bucket" ] && [ "$target_bucket" != "$source_bucket" ] || {
  echo "restore drill requires a dedicated bucket different from production source $source_bucket" >&2
  exit 1
}

restore_state_key=$(
  printf '%s\n%s\n' "$backup_id" "$target_bucket" |
    shasum -a 256 |
    awk '{print $1}'
)
state_root=${BRUNN_MANAGED_RESTORE_STATE_ROOT:-}
[ -n "$state_root" ] ||
  state_root=$(read_value BRUNN_MANAGED_RESTORE_STATE_ROOT)
case "$state_root" in
  /*)
    ;;
  *)
    echo "BRUNN_MANAGED_RESTORE_STATE_ROOT must name an absolute durable directory" >&2
    exit 64
    ;;
esac
case "$state_root" in
  /tmp|/tmp/*|/private/tmp|/private/tmp/*|/var/tmp|/var/tmp/*)
    echo "managed restore state must not use an ephemeral temporary directory" >&2
    exit 64
    ;;
esac
state_dir="$state_root/$restore_state_key"
mkdir -p "$state_dir"
mapping="$state_dir/restore-map.json"
project="brunn-managed-restore-$(printf '%s' "$restore_state_key" | cut -c1-16)"

running_containers=$(docker ps -q)
if [ -n "$running_containers" ]; then
  docker inspect $running_containers |
    jq -r '.[] | [.Id, .Name[1:], .RestartCount] | @tsv' >"$host_baseline"
else
  : >"$host_baseline"
fi

compose() {
  docker compose \
    --project-name "$project" \
    --env-file "$env_file" \
    --file "$root/compose.yaml" \
    --file "$production_overlay" \
    --file "$managed_overlay" \
    --file "$restore_overlay" \
    "$@"
}

cleanup() {
  status=$?
  trap - EXIT INT TERM
  if [ "$status" -ne 0 ]; then
    echo "restore drill failed; preserving project $project, target bucket $target_bucket, and resumable state $state_dir" >&2
    rm -rf "$temp_dir"
    if [ "$operation_lock_acquired" = true ]; then
      rmdir "$operation_lock_dir" >/dev/null 2>&1 || true
    fi
    exit "$status"
  fi
  compose down --volumes --remove-orphans >/dev/null 2>&1 || true
  rm -rf "$temp_dir"
  if [ "$operation_lock_acquired" = true ]; then
    rmdir "$operation_lock_dir" >/dev/null 2>&1 || true
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

mkdir "$operation_lock_dir" 2>/dev/null || {
  echo "another backup, restore, or prune operation holds $operation_lock_dir" >&2
  exit 1
}
operation_lock_acquired=true
"$root/scripts/verify-managed-backup.sh" "$backup_dir"

wait_healthy() {
  service=$1
  attempts=0
  while [ "$attempts" -lt 90 ]; do
    container_id=$(compose ps -q "$service")
    if [ -n "$container_id" ] &&
      [ "$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$container_id")" = "healthy" ]; then
      return 0
    fi
    attempts=$((attempts + 1))
    sleep 1
  done
  echo "$service did not become healthy during managed restore drill" >&2
  return 1
}

verify_host_stability() {
  tab=$(printf '\t')
  while IFS="$tab" read -r container_id container_name restart_count; do
    [ -n "$container_id" ] || continue
    state=$(
      docker inspect \
        --format '{{.State.Status}}|{{.RestartCount}}|{{.State.OOMKilled}}' \
        "$container_id" 2>/dev/null || true
    )
    [ "$state" = "running|$restart_count|false" ] || {
      echo "restore drill disrupted pre-existing container $container_name: $state" >&2
      return 1
    }
  done <"$host_baseline"
}

echo "starting isolated PostgreSQL restore target"
compose up -d db >/dev/null
wait_healthy db
db_container=$(compose ps -q db)
db_user=$(docker inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$db_container" |
  awk -F= '$1 == "POSTGRES_USER" {sub(/^[^=]*=/, ""); print; exit}')
db_name=$(docker inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$db_container" |
  awk -F= '$1 == "POSTGRES_DB" {sub(/^[^=]*=/, ""); print; exit}')
[ -n "$db_user" ] && [ -n "$db_name" ] || {
  echo "could not resolve isolated restore database identity" >&2
  exit 1
}

echo "restoring PostgreSQL snapshot"
docker exec -i "$db_container" pg_restore \
  --username "$db_user" --dbname "$db_name" --clean --if-exists \
  --exit-on-error --single-transaction <"$backup_dir/postgres.dump"

docker exec -i "$db_container" psql \
  --username "$db_user" --dbname "$db_name" --quiet --no-psqlrc \
  <"$root/scripts/db-inventory.sql" >"$temp_dir/db-inventory-restored.txt"
diff -u "$backup_dir/db-inventory.txt" "$temp_dir/db-inventory-restored.txt"
docker exec "$db_container" /usr/local/bin/brunn-postgres-healthcheck

echo "qualifying the dedicated managed S3 drill bucket"
ENV_FILE="$env_file" \
  COMPOSE_PROJECT_NAME="$project" \
  COMPOSE_OVERRIDE_FILE="$production_overlay" \
  COMPOSE_MANAGED_S3_FILE="$managed_overlay" \
  "$root/scripts/qualify-object-store.sh" >/dev/null

echo "restoring and verifying every object version"
compose run --rm -T --no-deps \
  --user "$(id -u):$(id -g)" \
  --volume "$backup_dir:/backup:ro" \
  --volume "$state_dir:/restore-state" \
  migrate object-store-backup restore \
  --input /backup/object-store \
  --mapping /restore-state/restore-map.json >"$temp_dir/object-restore.json"

echo "running current migrations after restoring the database and objects"
compose run --rm migrate >/dev/null

echo "pinning legacy NULL database references to restored target versions"
compose run --rm -T migrate object-store-backup pin-database \
  >"$temp_dir/database-object-pinning.json"
jq -e '
  (.asset_versions_pinned | type == "number" and . >= 0)
  and (.upload_canonical_versions_pinned | type == "number" and . >= 0)
  and (.account_export_versions_pinned | type == "number" and . >= 0)
  and (.objects_stream_verified | type == "number" and . >= 0)
' "$temp_dir/database-object-pinning.json" >/dev/null

echo "remapping provider-specific object version IDs in PostgreSQL"
compose run --rm -T --no-deps \
  --user "$(id -u):$(id -g)" \
  --volume "$backup_dir:/backup:ro" \
  --volume "$state_dir:/restore-state:ro" \
  migrate object-store-backup remap-database \
  --input /backup/object-store \
  --mapping /restore-state/restore-map.json >"$temp_dir/database-remap.json"
jq -e '
  (.mapping_entries | type == "number" and . >= 0)
  and (.database_references_verified | type == "number" and . >= 0)
' "$temp_dir/database-remap.json" >/dev/null || {
  echo "database object-version remap did not report complete reference verification" >&2
  exit 1
}

echo "re-verifying restored object bytes after database remap"
compose run --rm -T --no-deps \
  --user "$(id -u):$(id -g)" \
  --volume "$backup_dir:/backup:ro" \
  --volume "$state_dir:/restore-state" \
  migrate object-store-backup restore \
  --input /backup/object-store \
  --mapping /restore-state/restore-map.json >"$temp_dir/post-remap-object-verify.json"

echo "verifying restored database references against exact object bytes"
compose run --rm -T migrate object-store-backup verify-database \
  >"$temp_dir/database-object-verification.json"
jq -e '
  (.references_verified | type == "number" and . >= 0)
  and (.unique_object_versions_verified | type == "number" and . >= 0)
  and (.logical_bytes_verified | type == "number" and . >= 0)
' "$temp_dir/database-object-verification.json" >/dev/null
object_reference_integrity_verified=true

docker exec -i "$db_container" psql \
  --username "$db_user" --dbname "$db_name" --quiet --no-psqlrc \
  <"$root/scripts/db-inventory.sql" >"$temp_dir/db-inventory-migrated.txt"
docker exec "$db_container" /usr/local/bin/brunn-postgres-healthcheck

echo "running no-op migrations and restored API/worker readiness"
compose run --rm migrate >/dev/null
compose up -d api worker >/dev/null
wait_healthy api
worker_container=$(compose ps -q worker)
[ -n "$worker_container" ] &&
  [ "$(docker inspect --format '{{.State.Running}}' "$worker_container")" = "true" ] || {
  echo "worker is not running against the restored database" >&2
  exit 1
}
verify_host_stability

compose stop --timeout 10 api worker >/dev/null
compose run --rm -T --no-deps \
  --user "$(id -u):$(id -g)" \
  --volume "$state_dir:/restore-state" \
  migrate object-store-backup cleanup-restore \
  --mapping /restore-state/restore-map.json >/dev/null
rm -rf "$state_dir"
mapping=

completed_epoch=$(date +%s)
rto_seconds=$((completed_epoch - started_epoch))
manifest_sha256=$(shasum -a 256 "$backup_dir/manifest.json" |
  awk '{print "sha256:" $1}')
receipt="$backup_dir/restore-drill-receipt.json"
receipt_temp="$backup_dir/.restore-drill-receipt.$$.tmp"
jq -n \
  --arg backup_id "$backup_id" \
  --arg manifest_sha256 "$manifest_sha256" \
  --arg completed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg target_bucket "$target_bucket" \
  --arg project "$project" \
  --argjson rto_seconds "$rto_seconds" \
  --argjson object_reference_integrity "$object_reference_integrity_verified" \
  '{
    format: "brunn-restore-drill-receipt@v1",
    status: "pass",
    backup_id: $backup_id,
    backup_manifest_sha256: $manifest_sha256,
    completed_at: $completed_at,
    drill_kind: "managed-s3",
    target_bucket: $target_bucket,
    project: $project,
    rto_seconds: $rto_seconds,
    checks: {
      database_restore: true,
      object_restore: true,
      object_reference_integrity: $object_reference_integrity,
      api_ready: true,
      worker_ready: true
    }
  }' >"$receipt_temp"
chmod 0600 "$receipt_temp"
mv "$receipt_temp" "$receipt"
"$root/scripts/verify-restore-drill-receipt.sh" "$backup_dir"
echo "managed S3 restore drill PASS: project=$project source_bucket=$source_bucket target_bucket=$target_bucket rto_seconds=$rto_seconds"

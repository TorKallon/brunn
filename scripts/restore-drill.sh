#!/bin/sh
set -eu
umask 077

usage() {
  echo "usage: $0 BACKUP_DIR" >&2
  exit 64
}

[ "$#" -eq 1 ] || usage
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
backup_dir=$(CDPATH= cd -- "$1" && pwd)
backup_root=$(dirname "$backup_dir")
env_file=${ENV_FILE:-"$root/.env"}
project="brunn-restore-$(date -u +%Y%m%d%H%M%S)-$$"
compose_file="$root/compose.yaml"
compose_override_file=${COMPOSE_OVERRIDE_FILE:-}
override_file="$root/compose.restore-drill.yaml"
started_epoch=$(date +%s)
temp_dir=$(mktemp -d "${TMPDIR:-/tmp}/brunn-restore-drill.XXXXXX")
host_baseline="$temp_dir/host-containers.tsv"
operation_lock_dir="$backup_root/.recovery-operation.lock"
operation_lock_acquired=false
object_reference_integrity_verified=false

compose() {
  if [ -n "$compose_override_file" ]; then
    docker compose \
      --project-name "$project" \
      --env-file "$env_file" \
      --file "$compose_file" \
      --file "$compose_override_file" \
      --file "$override_file" \
      "$@"
  else
    docker compose \
      --project-name "$project" \
      --env-file "$env_file" \
      --file "$compose_file" \
      --file "$override_file" \
      "$@"
  fi
}

cleanup() {
  status=$?
  trap - EXIT INT TERM
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

"$root/scripts/verify-backup.sh" "$backup_dir"
running_containers=$(docker ps -q)
if [ -n "$running_containers" ]; then
  docker inspect $running_containers |
    jq -r '.[] | [.Id, .Name[1:], .RestartCount] | @tsv' \
      >"$host_baseline"
else
  : >"$host_baseline"
fi

wait_healthy() {
  service=$1
  attempts=0
  while [ "$attempts" -lt 60 ]; do
    container_id=$(compose ps -q "$service")
    if [ -n "$container_id" ] &&
      [ "$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$container_id")" = "healthy" ]; then
      return 0
    fi
    attempts=$((attempts + 1))
    sleep 1
  done
  echo "$service did not become healthy during restore drill" >&2
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

echo "starting isolated restore stores"
compose up -d db minio >/dev/null
wait_healthy db
wait_healthy minio

db_container=$(compose ps -q db)
minio_container=$(compose ps -q minio)
docker exec "$db_container" /usr/local/bin/brunn-postgres-healthcheck
db_user=$(docker inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$db_container" |
  awk -F= '$1 == "POSTGRES_USER" {sub(/^[^=]*=/, ""); print; exit}')
db_name=$(docker inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$db_container" |
  awk -F= '$1 == "POSTGRES_DB" {sub(/^[^=]*=/, ""); print; exit}')
[ -n "$db_user" ] && [ -n "$db_name" ] || {
  echo "could not resolve restored database identity" >&2
  exit 1
}
minio_volume=$(docker inspect --format \
  '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Name}}{{end}}{{end}}' \
  "$minio_container")
[ -n "$minio_volume" ] || {
  echo "could not identify restored MinIO volume" >&2
  exit 1
}

echo "restoring all MinIO versions"
compose stop --timeout 30 minio >/dev/null
docker run --rm --network none \
  --volume "$minio_volume:/target" \
  --volume "$backup_dir:/backup:ro" \
  alpine:3.23@sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40 \
  sh -ec '
    find /target -mindepth 1 -delete
    tar --numeric-owner --directory /target --extract \
      --file /backup/minio-data.tar
  '
compose up -d minio >/dev/null
wait_healthy minio

echo "restoring PostgreSQL"
docker exec -i \
  "$db_container" pg_restore \
  --username "$db_user" \
  --dbname "$db_name" \
  --clean \
  --if-exists \
  --exit-on-error \
  --single-transaction \
  <"$backup_dir/postgres.dump"

docker exec -i "$db_container" psql \
  --username "$db_user" \
  --dbname "$db_name" \
  --quiet \
  --no-psqlrc \
  <"$root/scripts/db-inventory.sql" \
  >"$temp_dir/db-inventory.txt"
diff -u "$backup_dir/db-inventory.txt" "$temp_dir/db-inventory.txt"
docker exec "$db_container" /usr/local/bin/brunn-postgres-healthcheck

echo "verifying restored object-version inventory"
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
' | LC_ALL=C sort >"$temp_dir/minio-versions.jsonl"
diff -u \
  "$backup_dir/minio-versions.jsonl" \
  "$temp_dir/minio-versions.jsonl"

echo "running no-op migrations and restored API/worker healthcheck"
compose run --rm migrate >/dev/null
echo "pinning any legacy restored database references"
compose run --rm -T migrate object-store-backup pin-database \
  >"$temp_dir/database-object-pinning.json"
jq -e '
  (.asset_versions_pinned | type == "number" and . >= 0)
  and (.upload_canonical_versions_pinned | type == "number" and . >= 0)
  and (.account_export_versions_pinned | type == "number" and . >= 0)
  and (.objects_stream_verified | type == "number" and . >= 0)
' "$temp_dir/database-object-pinning.json" >/dev/null
echo "verifying every restored database reference against exact object bytes"
compose run --rm -T migrate object-store-backup verify-database \
  >"$temp_dir/database-object-verification.json"
jq -e '
  (.references_verified | type == "number" and . >= 0)
  and (.unique_object_versions_verified | type == "number" and . >= 0)
  and (.logical_bytes_verified | type == "number" and . >= 0)
' "$temp_dir/database-object-verification.json" >/dev/null
object_reference_integrity_verified=true

operator_marker=$(date -u +%Y%m%d%H%M%S)-$$
operator_provision="$temp_dir/operator-provision.json"
operator_recovery="$temp_dir/operator-recovery.json"
operator_compromise_recovery="$temp_dir/operator-compromise-recovery.json"
compose run --rm -T migrate operator provision-user \
  --external-ref "restore-drill:$operator_marker" \
  --display-name "Restore drill operator user" \
  --credential-name "Restore drill initial owner" \
  >"$operator_provision"
jq -e '
  (.user.id | test("^user:[0-9a-f-]{36}$"))
  and (.credential.token | test("^sl_[A-Za-z0-9_-]{40,}$"))
  and (.credential.token_status == "issued_once")
' "$operator_provision" >/dev/null
operator_user_id=$(jq -r '.user.id' "$operator_provision")
compose run --rm -T migrate operator recover-user \
  --user-id "$operator_user_id" \
  --credential-name "Restore drill recovered owner" \
  >"$operator_recovery"
jq -e --arg user_id "$operator_user_id" '
  (.user.id == $user_id)
  and (.credential.token | test("^sl_[A-Za-z0-9_-]{40,}$"))
  and (.credential.token_status == "issued_once")
  and (.recovery_mode == "lost")
  and (.revoked_existing_owner_credentials == 0)
' "$operator_recovery" >/dev/null
provision_token=$(jq -r '.credential.token' "$operator_provision")
recovery_token=$(jq -r '.credential.token' "$operator_recovery")
provision_hash=$(printf '%s' "$provision_token" | shasum -a 256 | awk '{print $1}')
recovery_hash=$(printf '%s' "$recovery_token" | shasum -a 256 | awk '{print $1}')
authenticated_credentials=$(docker exec "$db_container" psql \
  --username "$db_user" \
  --dbname "$db_name" \
  --tuples-only \
  --no-align \
  --no-psqlrc \
  --command "SELECT count(*) FROM brunn.api_credentials WHERE token_hash IN ('$provision_hash','$recovery_hash') AND disabled_at IS NULL" |
  tr -d '[:space:]')
[ "$authenticated_credentials" = "2" ] || {
  echo "operator provision/recovery credentials were not durably created" >&2
  exit 1
}

compose run --rm -T migrate operator recover-user \
  --user-id "$operator_user_id" \
  --credential-name "Restore drill compromised-token recovery" \
  --revoke-existing-owner-credentials \
  >"$operator_compromise_recovery"
jq -e --arg user_id "$operator_user_id" '
  (.user.id == $user_id)
  and (.credential.token | test("^sl_[A-Za-z0-9_-]{40,}$"))
  and (.credential.token_status == "issued_once")
  and (.recovery_mode == "compromised")
  and (.revoked_existing_owner_credentials == 2)
' "$operator_compromise_recovery" >/dev/null
compromise_token=$(jq -r '.credential.token' "$operator_compromise_recovery")
compromise_hash=$(printf '%s' "$compromise_token" | shasum -a 256 | awk '{print $1}')
credential_states=$(docker exec "$db_container" psql \
  --username "$db_user" \
  --dbname "$db_name" \
  --tuples-only \
  --no-align \
  --no-psqlrc \
  --command "SELECT count(*) FILTER (WHERE disabled_at IS NULL),count(*) FILTER (WHERE disabled_at IS NOT NULL) FROM brunn.api_credentials WHERE token_hash IN ('$provision_hash','$recovery_hash','$compromise_hash')" |
  tr -d '[:space:]')
[ "$credential_states" = "1|2" ] || {
  echo "compromised-token recovery did not atomically replace existing owners" >&2
  exit 1
}

compose up -d api worker >/dev/null
wait_healthy api
worker_container=$(compose ps -q worker)
[ -n "$worker_container" ] &&
  [ "$(docker inspect --format '{{.State.Running}}' "$worker_container")" = "true" ] || {
  echo "worker is not running against the restored database" >&2
  exit 1
}
verify_host_stability

completed_epoch=$(date +%s)
rto_seconds=$((completed_epoch - started_epoch))
backup_id=$(jq -r '.backup_id' "$backup_dir/manifest.json")
manifest_sha256=$(shasum -a 256 "$backup_dir/manifest.json" |
  awk '{print "sha256:" $1}')
receipt="$backup_dir/restore-drill-receipt.json"
receipt_temp="$backup_dir/.restore-drill-receipt.$$.tmp"
jq -n \
  --arg backup_id "$backup_id" \
  --arg manifest_sha256 "$manifest_sha256" \
  --arg completed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg project "$project" \
  --argjson rto_seconds "$rto_seconds" \
  --argjson object_reference_integrity "$object_reference_integrity_verified" \
  '{
    format: "brunn-restore-drill-receipt@v1",
    status: "pass",
    backup_id: $backup_id,
    backup_manifest_sha256: $manifest_sha256,
    completed_at: $completed_at,
    drill_kind: "self-hosted-minio",
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
echo "restore drill PASS: project=$project rto_seconds=$rto_seconds operator_paths=verified"

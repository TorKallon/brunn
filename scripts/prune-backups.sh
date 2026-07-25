#!/bin/sh
set -eu
umask 077

usage() {
  echo "usage: $0 [--apply] BACKUP_ROOT" >&2
  exit 64
}

apply=false
if [ "${1:-}" = "--apply" ]; then
  apply=true
  shift
fi
[ "$#" -eq 1 ] || usage

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
backup_root=$1
[ -d "$backup_root" ] || {
  echo "backup root does not exist: $backup_root" >&2
  exit 1
}
backup_root=$(CDPATH= cd -- "$backup_root" && pwd)

record_watermark=${STRAYLIGHT_RECORD_BACKUP_WATERMARK:-false}
case "$record_watermark" in
  true|false)
    ;;
  *)
    echo "STRAYLIGHT_RECORD_BACKUP_WATERMARK must be true or false" >&2
    exit 64
    ;;
esac
[ "$record_watermark" != true ] || [ "$apply" = true ] || {
  echo "backup watermark recording requires --apply" >&2
  exit 64
}

now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
started_at=$now
expired=0
kept=0
skipped=0
protected=0
inventory=$(mktemp "${TMPDIR:-/tmp}/straylight-backup-prune.XXXXXX")
deleted_ids=$(mktemp "${TMPDIR:-/tmp}/straylight-backup-deleted.XXXXXX")
retained_times=$(mktemp "${TMPDIR:-/tmp}/straylight-backup-retained.XXXXXX")
operation_lock_dir="$backup_root/.recovery-operation.lock"
operation_lock_acquired=false
receipt_dir="$backup_root/prune-receipts"
receipt_id=$(date -u +%Y%m%dT%H%M%SZ)-$(uuidgen | tr '[:upper:]' '[:lower:]')
receipt_path=
receipt_checksum_path=
receipt_sha256=
receipt_initialized=false
receipt_finalized=false

oldest_retained_created_at() {
  : >"$retained_times"
  tab=$(printf '\t')
  while IFS="$tab" read -r backup_dir _format _backup_id created_at \
    _completed_at _expires_at _restore_drilled; do
    [ -d "$backup_dir" ] || continue
    printf '%s\n' "$created_at" >>"$retained_times"
  done <"$inventory"
  LC_ALL=C sort "$retained_times" | sed -n '1p'
}

write_prune_receipt() {
  status=$1
  oldest_retained=$(oldest_retained_created_at)
  deleted_json=$(jq -R -s 'split("\n") | map(select(length > 0))' \
    "$deleted_ids")
  receipt_name="$receipt_id.json"
  receipt_temp="$receipt_dir/.$receipt_name.tmp"
  checksum_temp="$receipt_dir/.$receipt_name.sha256.tmp"
  jq -n \
    --arg receipt_id "$receipt_id" \
    --arg started_at "$started_at" \
    --arg completed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg status "$status" \
    --arg backup_root "$backup_root" \
    --arg oldest_retained_created_at "$oldest_retained" \
    --argjson deleted_backup_ids "$deleted_json" \
    '{
      format: "straylight-backup-prune-receipt@v1",
      receipt_id: $receipt_id,
      status: $status,
      started_at: $started_at,
      completed_at: $completed_at,
      backup_root: $backup_root,
      deleted_backup_ids: $deleted_backup_ids,
      oldest_retained_created_at: (
        if $oldest_retained_created_at == ""
        then null
        else $oldest_retained_created_at
        end
      )
    }' >"$receipt_temp"
  chmod 0600 "$receipt_temp"
  digest=$(shasum -a 256 "$receipt_temp" | awk '{print $1}')
  printf '%s  %s\n' "$digest" "$receipt_name" >"$checksum_temp"
  chmod 0600 "$checksum_temp"
  mv "$receipt_temp" "$receipt_path"
  mv "$checksum_temp" "$receipt_checksum_path"
  receipt_sha256="sha256:$digest"
}

cleanup() {
  status=$?
  trap - EXIT INT TERM
  if [ "$apply" = true ] && [ "$receipt_initialized" = true ] &&
    [ "$receipt_finalized" != true ]; then
    write_prune_receipt partial >/dev/null 2>&1 || true
  fi
  rm -f "$inventory" "$deleted_ids" "$retained_times"
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
if [ "$apply" = true ]; then
  mkdir -p "$receipt_dir"
  chmod 0700 "$receipt_dir"
  receipt_path="$receipt_dir/$receipt_id.json"
  receipt_checksum_path="$receipt_path.sha256"
  receipt_initialized=true
  write_prune_receipt in_progress
fi

# Verify every recognized recovery point before deleting any of them. The
# inventory is frozen for this pruning pass so deletion order cannot change
# which backup protects another.
for backup_dir in "$backup_root"/*; do
  [ -d "$backup_dir" ] || continue
  [ "$backup_dir" != "$receipt_dir" ] || continue
  manifest="$backup_dir/manifest.json"
  if [ ! -f "$manifest" ]; then
    echo "skipping non-backup directory: $backup_dir" >&2
    skipped=$((skipped + 1))
    continue
  fi
  format=$(jq -r '.format // ""' "$manifest")
  backup_id=$(jq -r '.backup_id // ""' "$manifest")
  created_at=$(jq -r '.created_at // ""' "$manifest")
  completed_at=$(jq -r '.completed_at // .created_at // ""' "$manifest")
  expires_at=$(jq -r '.expires_at // ""' "$manifest")
  case "$format" in
    straylight-coordinated-backup@v2)
      verifier="$root/scripts/verify-backup.sh"
      ;;
    straylight-managed-s3-coordinated-backup@v1)
      verifier="$root/scripts/verify-managed-backup.sh"
      ;;
    *)
      echo "skipping backup without an enforced expiry contract: $backup_dir" >&2
      skipped=$((skipped + 1))
      continue
      ;;
  esac
  [ -n "$backup_id" ] && [ -n "$created_at" ] &&
    [ -n "$completed_at" ] && [ -n "$expires_at" ] || {
    echo "skipping backup without identity, creation, completion, or expiry: $backup_dir" >&2
    skipped=$((skipped + 1))
    continue
  }
  "$verifier" "$backup_dir" >/dev/null
  restore_drilled=false
  if "$root/scripts/verify-restore-drill-receipt.sh" "$backup_dir" \
    >/dev/null 2>&1; then
    restore_drilled=true
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$backup_dir" "$format" "$backup_id" "$created_at" \
    "$completed_at" "$expires_at" \
    "$restore_drilled" >>"$inventory"
done

tab=$(printf '\t')
while IFS="$tab" read -r backup_dir format backup_id created_at completed_at \
  expires_at restore_drilled; do
  [ -n "$backup_dir" ] || continue
  if [ "$expires_at" \> "$now" ]; then
    kept=$((kept + 1))
    continue
  fi
  expired=$((expired + 1))
  if ! awk -F '\t' \
    -v format="$format" \
    -v completed_at="$completed_at" \
    -v expires_at="$expires_at" \
    -v now="$now" '
      $2 == format && ($5 > completed_at || ($5 == completed_at && $6 > expires_at)) && $6 > now && $7 == "true" {
        found=1
        exit
      }
      END { exit(found ? 0 : 1) }
    ' "$inventory"; then
    protected=$((protected + 1))
    echo "keeping expired backup without a newer verified unexpired recovery point: $backup_dir" >&2
    continue
  fi
  if [ "$apply" = true ]; then
    rm -rf "$backup_dir"
    [ ! -e "$backup_dir" ] || {
      echo "backup still exists after deletion attempt: $backup_dir" >&2
      exit 1
    }
    printf '%s\n' "$backup_id" >>"$deleted_ids"
    write_prune_receipt in_progress
    echo "deleted expired backup: $backup_dir"
  else
    echo "would delete expired backup: $backup_dir"
  fi
done <"$inventory"

echo "backup retention scan: expired=$expired kept=$kept skipped=$skipped apply=$apply protected=$protected"
if [ "$apply" = true ]; then
  if [ "$protected" -eq 0 ]; then
    write_prune_receipt pass
  else
    write_prune_receipt partial
  fi
  receipt_finalized=true
  echo "prune receipt: $receipt_path checksum=$receipt_sha256"
fi
[ "$protected" -eq 0 ] || exit 1

if [ "$record_watermark" = true ]; then
  oldest_retained=$(oldest_retained_created_at)
  [ -n "$oldest_retained" ] || {
    echo "cannot record a backup watermark without a retained verified backup" >&2
    exit 1
  }
  env_file=${ENV_FILE:-"$root/production.env"}
  [ -f "$env_file" ] || {
    echo "cannot record backup watermark without environment file: $env_file" >&2
    exit 1
  }
  project=${COMPOSE_PROJECT_NAME:-straylight}
  compose_file=${COMPOSE_FILE:-"$root/compose.yaml"}
  production_overlay=${COMPOSE_OVERRIDE_FILE:-"$root/compose.production.yaml"}
  managed_overlay=${COMPOSE_MANAGED_S3_FILE:-"$root/compose.managed-s3.yaml"}
  object_store_mode=$(awk -F= '
    $1 == "STRAYLIGHT_OBJECT_STORE_MODE" {
      sub(/^[^=]*=/, "")
      print
      exit
    }
  ' "$env_file")
  object_store_mode=${object_store_mode:-self-hosted-minio}
  watermark_source=${STRAYLIGHT_BACKUP_WATERMARK_SOURCE:-"prune-backups:$receipt_id"}
  watermark_result="$receipt_dir/$receipt_id.watermark.json"
  watermark_temp="$receipt_dir/.$receipt_id.watermark.tmp"
  case "$object_store_mode" in
    self-hosted-minio)
      docker compose \
        --project-name "$project" \
        --env-file "$env_file" \
        --file "$compose_file" \
        --file "$production_overlay" \
        run --rm -T migrate operator record-backup-watermark \
        --oldest-retained-created-at "$oldest_retained" \
        --receipt-sha256 "$receipt_sha256" \
        --source "$watermark_source" >"$watermark_temp"
      ;;
    managed-s3)
      docker compose \
        --project-name "$project" \
        --env-file "$env_file" \
        --file "$compose_file" \
        --file "$production_overlay" \
        --file "$managed_overlay" \
        run --rm -T migrate operator record-backup-watermark \
        --oldest-retained-created-at "$oldest_retained" \
        --receipt-sha256 "$receipt_sha256" \
        --source "$watermark_source" >"$watermark_temp"
      ;;
    *)
      echo "cannot record watermark for unsupported object-store mode: $object_store_mode" >&2
      exit 1
      ;;
  esac
  jq -e \
    --arg oldest "$oldest_retained" \
    --arg receipt_sha256 "$receipt_sha256" \
    --arg source "$watermark_source" '
      .status == "recorded"
      and .oldest_retained_created_at == $oldest
      and .receipt_sha256 == $receipt_sha256
      and .source == $source
      and (.account_deletions_verified | type == "number" and . >= 0)
    ' "$watermark_temp" >/dev/null || {
    echo "backup watermark command returned an invalid receipt" >&2
    exit 1
  }
  chmod 0600 "$watermark_temp"
  mv "$watermark_temp" "$watermark_result"
  echo "backup watermark recorded: $watermark_result"
fi

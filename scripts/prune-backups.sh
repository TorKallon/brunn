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

now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
expired=0
kept=0
skipped=0

for backup_dir in "$backup_root"/*; do
  [ -d "$backup_dir" ] || continue
  manifest="$backup_dir/manifest.json"
  if [ ! -f "$manifest" ]; then
    echo "skipping non-backup directory: $backup_dir" >&2
    skipped=$((skipped + 1))
    continue
  fi
  format=$(jq -r '.format // ""' "$manifest")
  expires_at=$(jq -r '.expires_at // ""' "$manifest")
  if [ "$format" != "straylight-coordinated-backup@v2" ] ||
    [ -z "$expires_at" ]; then
    echo "skipping backup without enforced v2 expiry: $backup_dir" >&2
    skipped=$((skipped + 1))
    continue
  fi
  "$root/scripts/verify-backup.sh" "$backup_dir" >/dev/null
  if [ "$expires_at" \> "$now" ]; then
    kept=$((kept + 1))
    continue
  fi
  expired=$((expired + 1))
  if [ "$apply" = true ]; then
    rm -rf "$backup_dir"
    echo "deleted expired backup: $backup_dir"
  else
    echo "would delete expired backup: $backup_dir"
  fi
done

echo "backup retention scan: expired=$expired kept=$kept skipped=$skipped apply=$apply"

#!/bin/sh
set -eu

[ "$#" -eq 1 ] || {
  echo "usage: $0 BACKUP_DIR" >&2
  exit 64
}
backup_dir=$1
manifest="$backup_dir/manifest.json"
receipt="$backup_dir/restore-drill-receipt.json"
[ -f "$manifest" ] && [ -f "$receipt" ] || exit 1

backup_id=$(jq -r '.backup_id // ""' "$manifest")
manifest_sha256=$(shasum -a 256 "$manifest" | awk '{print "sha256:" $1}')
jq -e \
  --arg backup_id "$backup_id" \
  --arg manifest_sha256 "$manifest_sha256" '
  .format == "straylight-restore-drill-receipt@v1"
  and .status == "pass"
  and .backup_id == $backup_id
  and .backup_manifest_sha256 == $manifest_sha256
  and (.completed_at | type == "string"
    and test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T"))
  and (.rto_seconds | type == "number" and . >= 0)
  and (.checks.database_restore == true)
  and (.checks.object_restore == true)
  and (.checks.object_reference_integrity == true)
  and (.checks.api_ready == true)
  and (.checks.worker_ready == true)
' "$receipt" >/dev/null

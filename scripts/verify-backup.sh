#!/bin/sh
set -eu

usage() {
  echo "usage: $0 BACKUP_DIR" >&2
  exit 64
}

[ "$#" -eq 1 ] || usage
backup_dir=$1

[ -d "$backup_dir" ] || {
  echo "backup directory does not exist: $backup_dir" >&2
  exit 1
}

for required in manifest.json CHECKSUMS.sha256 postgres.dump minio-data.tar \
  db-inventory.txt minio-versions.jsonl; do
  [ -f "$backup_dir/$required" ] || {
    echo "backup is missing $required" >&2
    exit 1
  }
done

(cd "$backup_dir" && shasum -a 256 -c CHECKSUMS.sha256)

format=$(jq -r '.format' "$backup_dir/manifest.json")
case "$format" in
  straylight-coordinated-backup@v1)
    ;;
  straylight-coordinated-backup@v2)
    jq -e '
      (.backup_id | type == "string" and length > 0)
      and (.created_at | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T"))
      and (.completed_at | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T"))
      and (.expires_at | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T"))
      and (.retention_days | type == "number" and . >= 1 and . <= 90)
      and (.expires_at > .created_at)
      and (.consistency.writers_stopped == true)
    ' "$backup_dir/manifest.json" >/dev/null || {
      echo "backup manifest does not satisfy the v2 contract" >&2
      exit 1
    }
    grep -Eq '[[:space:]]manifest\.json$' "$backup_dir/CHECKSUMS.sha256" || {
      echo "v2 backup checksum manifest does not protect manifest.json" >&2
      exit 1
    }
    if jq -e '.runtime_identity != null' "$backup_dir/manifest.json" >/dev/null; then
      for runtime_file in runtime-images.json compose-service-hashes.txt; do
        [ -f "$backup_dir/$runtime_file" ] || {
          echo "backup runtime identity is missing $runtime_file" >&2
          exit 1
        }
        grep -Eq "[[:space:]]$runtime_file$" "$backup_dir/CHECKSUMS.sha256" || {
          echo "backup checksum manifest does not protect $runtime_file" >&2
          exit 1
        }
      done
      invariants=$(jq -r '.runtime_identity.database_invariants // ""' \
        "$backup_dir/manifest.json")
      if [ -n "$invariants" ]; then
        [ "$invariants" = "database-invariants.json" ] || {
          echo "unsupported database invariants path: $invariants" >&2
          exit 1
        }
        [ -f "$backup_dir/$invariants" ] || {
          echo "backup runtime identity is missing $invariants" >&2
          exit 1
        }
        grep -Eq "[[:space:]]$invariants$" "$backup_dir/CHECKSUMS.sha256" || {
          echo "backup checksum manifest does not protect $invariants" >&2
          exit 1
        }
        jq -e '
          .safe == true
          and .data_checksums == "on"
          and .locale_provider == "b"
          and .locale == "C.UTF-8"
          and .recorded_collation_version == .actual_collation_version
        ' "$backup_dir/$invariants" >/dev/null || {
          echo "backup database invariants are unsafe" >&2
          exit 1
        }
      fi
    fi
    ;;
  *)
    echo "unsupported backup format: $format" >&2
    exit 1
    ;;
esac

echo "backup verified: $backup_dir"

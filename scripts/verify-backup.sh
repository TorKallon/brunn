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
  brunn-coordinated-backup@v1)
    ;;
  brunn-coordinated-backup@v2)
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
      pinning=$(jq -r '.runtime_identity.database_object_pinning // ""' \
        "$backup_dir/manifest.json")
      verification=$(jq -r '.runtime_identity.database_object_verification // ""' \
        "$backup_dir/manifest.json")
      for evidence in "$pinning" "$verification"; do
        [ -n "$evidence" ] || continue
        case "$evidence" in
          database-object-pinning.json|database-object-verification.json)
            ;;
          *)
            echo "unsupported database object evidence path: $evidence" >&2
            exit 1
            ;;
        esac
        [ -f "$backup_dir/$evidence" ] || {
          echo "backup runtime identity is missing $evidence" >&2
          exit 1
        }
        grep -Eq "[[:space:]]$evidence$" "$backup_dir/CHECKSUMS.sha256" || {
          echo "backup checksum manifest does not protect $evidence" >&2
          exit 1
        }
      done
      if [ -n "$pinning" ]; then
        jq -e '
          (.asset_versions_pinned | type == "number" and . >= 0)
          and (.upload_canonical_versions_pinned | type == "number" and . >= 0)
          and (.account_export_versions_pinned | type == "number" and . >= 0)
          and (.objects_stream_verified | type == "number" and . >= 0)
        ' "$backup_dir/$pinning" >/dev/null || {
          echo "backup database object-pinning evidence is invalid" >&2
          exit 1
        }
      fi
      if [ -n "$verification" ]; then
        jq -e '
          (.references_verified | type == "number" and . >= 0)
          and (.unique_object_versions_verified | type == "number" and . >= 0)
          and (.logical_bytes_verified | type == "number" and . >= 0)
        ' "$backup_dir/$verification" >/dev/null || {
          echo "backup database object-verification evidence is invalid" >&2
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

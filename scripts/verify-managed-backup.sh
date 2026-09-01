#!/bin/sh
set -eu

usage() {
  echo "usage: $0 BACKUP_DIR" >&2
  exit 64
}

[ "$#" -eq 1 ] || usage
backup_dir=$1
[ -d "$backup_dir" ] || {
  echo "managed backup directory does not exist: $backup_dir" >&2
  exit 1
}

for required in manifest.json CHECKSUMS.sha256 postgres.dump \
  db-inventory.txt database-invariants.json runtime-images.json \
  database-object-references.json compose-service-hashes.txt \
  object-store/manifest.json; do
  [ -f "$backup_dir/$required" ] || {
    echo "managed backup is missing $required" >&2
    exit 1
  }
done

(cd "$backup_dir" && shasum -a 256 -c CHECKSUMS.sha256)

jq -e '
  .format == "brunn-managed-s3-coordinated-backup@v1"
  and (.backup_id | type == "string" and length > 0)
  and (.created_at | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T"))
  and (.completed_at | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T"))
  and (.completed_at >= .created_at)
  and (.expires_at > .created_at)
  and (.expires_at > .completed_at)
  and (.retention_days | type == "number" and . >= 1 and . <= 90)
  and (.object_bucket | type == "string" and length > 0)
  and (.consistency.writers_stopped == true)
  and (.consistency.postgres_snapshot == "serializable-deferrable")
  and (.consistency.object_snapshot == "portable-all-versions")
  and (.object_archive == "object-store/manifest.json")
' "$backup_dir/manifest.json" >/dev/null || {
  echo "managed backup manifest does not satisfy its v1 contract" >&2
  exit 1
}

object_manifest="$backup_dir/object-store/manifest.json"
coordinated_bucket=$(jq -r '.object_bucket' "$backup_dir/manifest.json")
jq -e '
  . as $manifest
  | .format == "brunn-object-version-archive@v1"
  and .bucket_versioning == "Enabled"
  and (.source_bucket | type == "string" and length > 0)
  and (.entries | type == "array")
  and (.summary.object_versions ==
    ([.entries[] | select(.kind == "object_version")] | length))
  and (.summary.delete_markers ==
    ([.entries[] | select(.kind == "delete_marker")] | length))
  and (.summary.unique_blobs ==
    ([.entries[] | select(.kind == "object_version") | .sha256] | unique | length))
  and (.summary.logical_bytes ==
    (([.entries[] | select(.kind == "object_version") | .size_bytes] | add) // 0))
  and (([.entries[] | [.kind, .key, .source_version_id] | join("\u001f")]
    | unique | length) == (.entries | length))
  and ([.entries[].key] | unique | all(
    . as $key
    | ([$manifest.entries[] | select(.key == $key and .is_latest)] | length) == 1
  ))
  and (all(.entries[];
    (.key | type == "string" and length > 0)
    and (.source_version_id | type == "string" and length > 0)
    and (.is_latest | type == "boolean")
    and (
      (.kind == "object_version"
       and (.size_bytes | type == "number" and . >= 0)
       and (.sha256 | test("^sha256:[0-9a-f]{64}$")))
      or
      (.kind == "delete_marker"
       and (.size_bytes == null)
       and (.sha256 == null))
    )
  ))
' "$object_manifest" >/dev/null || {
  echo "managed object archive manifest is invalid" >&2
  exit 1
}
[ "$(jq -r '.source_bucket' "$object_manifest")" = "$coordinated_bucket" ] || {
  echo "managed backup object bucket differs between coordinated and object manifests" >&2
  exit 1
}

expected_file=$(mktemp "${TMPDIR:-/tmp}/brunn-managed-backup.XXXXXX")
trap 'rm -f "$expected_file"' EXIT INT TERM
jq -r '
  .entries[]
  | select(.kind == "object_version")
  | [(.sha256 | sub("^sha256:"; "")), (.size_bytes | tostring)]
  | @tsv
' "$object_manifest" | LC_ALL=C sort -u >"$expected_file"

expected_count=0
expected_stored_bytes=0
while IFS="$(printf '\t')" read -r digest size; do
  [ -n "$digest" ] || continue
  case "$digest" in
    *[!0-9a-f]*)
      echo "managed object archive contains an unsafe digest path" >&2
      exit 1
      ;;
  esac
  [ "${#digest}" -eq 64 ] || {
    echo "managed object archive contains an invalid digest" >&2
    exit 1
  }
  blob="$backup_dir/object-store/objects/$digest"
  [ -f "$blob" ] && [ ! -L "$blob" ] || {
    echo "managed object archive is missing safe blob $digest" >&2
    exit 1
  }
  actual_size=$(wc -c <"$blob" | tr -d '[:space:]')
  [ "$actual_size" = "$size" ] || {
    echo "managed object blob $digest has size $actual_size, expected $size" >&2
    exit 1
  }
  actual_digest=$(shasum -a 256 "$blob" | awk '{print $1}')
  [ "$actual_digest" = "$digest" ] || {
    echo "managed object blob $digest failed SHA-256 verification" >&2
    exit 1
  }
  expected_count=$((expected_count + 1))
  expected_stored_bytes=$((expected_stored_bytes + actual_size))
done <"$expected_file"

actual_count=$(
  find "$backup_dir/object-store/objects" -mindepth 1 -maxdepth 1 \
    -type f ! -name '.partial-*' | wc -l | tr -d '[:space:]'
)
[ "$actual_count" = "$expected_count" ] || {
  echo "managed object archive has $actual_count blobs, expected $expected_count" >&2
  exit 1
}
[ "$(jq -r '.summary.stored_bytes' "$object_manifest")" = "$expected_stored_bytes" ] || {
  echo "managed object archive stored-byte total does not match its blobs" >&2
  exit 1
}
jq -e '.safe == true' "$backup_dir/database-invariants.json" >/dev/null || {
  echo "managed backup database invariants are unsafe" >&2
  exit 1
}

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
      echo "unsupported managed database object evidence path: $evidence" >&2
      exit 1
      ;;
  esac
  [ -f "$backup_dir/$evidence" ] || {
    echo "managed backup is missing $evidence" >&2
    exit 1
  }
  grep -Eq "[[:space:]]$evidence$" "$backup_dir/CHECKSUMS.sha256" || {
    echo "managed backup checksum manifest does not protect $evidence" >&2
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
    echo "managed backup database object-pinning evidence is invalid" >&2
    exit 1
  }
fi
if [ -n "$verification" ]; then
  jq -e '
    (.references_verified | type == "number" and . >= 0)
    and (.unique_object_versions_verified | type == "number" and . >= 0)
    and (.logical_bytes_verified | type == "number" and . >= 0)
  ' "$backup_dir/$verification" >/dev/null || {
    echo "managed backup database object-verification evidence is invalid" >&2
    exit 1
  }
fi

database_references="$backup_dir/database-object-references.json"
jq -e --arg bucket "$coordinated_bucket" '
  .format == "brunn-database-object-references@v1"
  and .object_bucket == $bucket
  and (.references | type == "array")
  and (.summary.reference_count == (.references | length))
  and (.summary.distinct_object_versions ==
    ([.references[] | [.object_key, .object_version_id] | join("\u001f")]
      | unique | length))
  and all(.references[];
    (.relation | type == "string" and length > 0)
    and (.user_id | type == "string" and length > 0)
    and .bucket == $bucket
    and (.object_key | type == "string" and length > 0)
    and (.object_version_id | type == "string" and length > 0
      and . != "null")
    and (.content_hash | test("^sha256:[0-9a-f]{64}$"))
    and (.size_bytes | type == "number" and . >= 0)
  )
' "$database_references" >/dev/null || {
  echo "managed backup database object-reference inventory is invalid" >&2
  exit 1
}

jq -n -e \
  --slurpfile database "$database_references" \
  --slurpfile objects "$object_manifest" '
  all($database[0].references[]; . as $reference |
    ([
      $objects[0].entries[]
      | select(
          .kind == "object_version"
          and .key == $reference.object_key
          and .source_version_id == $reference.object_version_id
          and .sha256 == $reference.content_hash
          and .size_bytes == $reference.size_bytes
        )
    ] | length) == 1
  )
' >/dev/null || {
  echo "managed backup omits or changes an authoritative database object reference" >&2
  exit 1
}

echo "managed coordinated backup verified: $backup_dir"

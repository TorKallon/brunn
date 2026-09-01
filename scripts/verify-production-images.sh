#!/bin/sh
set -eu

usage() {
  echo "usage: $0 ENV_FILE" >&2
  exit 64
}

[ "$#" -eq 1 ] || usage
env_file=$1
[ -f "$env_file" ] || {
  echo "production environment file does not exist: $env_file" >&2
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

revision=$(read_value BRUNN_RELEASE_REVISION)
object_store_mode=$(read_value BRUNN_OBJECT_STORE_MODE)
object_store_mode=${object_store_mode:-self-hosted-minio}
image_names="BRUNN_DATABASE_IMAGE BRUNN_API_IMAGE
BRUNN_WEB_IMAGE BRUNN_MCP_IMAGE BRUNN_EDGE_IMAGE"
if [ "$object_store_mode" = "self-hosted-minio" ]; then
  image_names="$image_names BRUNN_OBJECT_STORE_CLIENT_IMAGE"
fi
for name in $image_names; do
  image=$(read_value "$name")
  [ -n "$image" ] || {
    echo "$name is missing" >&2
    exit 1
  }
  docker image inspect "$image" >/dev/null 2>&1 || {
    echo "$name is not loaded locally: $image" >&2
    exit 1
  }
  image_revision=$(
    docker image inspect \
      --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' \
      "$image"
  )
  [ "$image_revision" = "$revision" ] || {
    echo "$name revision label $image_revision does not match $revision" >&2
    exit 1
  }
done

echo "production image identity valid: revision=$revision"

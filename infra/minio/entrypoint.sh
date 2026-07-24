#!/bin/sh
set -eu

load_secret() {
  variable=$1
  file_variable="${variable}_FILE"
  eval "value=\${${variable}:-}"
  eval "file=\${${file_variable}:-}"
  if [ -n "$value" ] && [ -n "$file" ]; then
    echo "$variable and $file_variable cannot both be set" >&2
    exit 1
  fi
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

exec /usr/local/bin/minio "$@"

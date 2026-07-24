#!/bin/sh
set -eu
umask 077

usage() {
  echo "usage: $0 HOSTNAME" >&2
  exit 64
}

[ "$#" -eq 1 ] || usage
host=$1
case "$host" in
  *://*|*/*|*:*|'')
    echo "hostname must not contain a scheme, path, or port" >&2
    exit 64
    ;;
esac

temp_dir=$(mktemp -d "${TMPDIR:-/tmp}/straylight-public-health.XXXXXX")
cleanup() {
  status=$?
  trap - EXIT INT TERM
  rm -rf "$temp_dir"
  exit "$status"
}
trap cleanup EXIT INT TERM

headers="$temp_dir/headers"
body="$temp_dir/ready.json"
curl --fail --silent --show-error \
  --proto '=https' \
  --tlsv1.2 \
  --connect-timeout 10 \
  --max-time 30 \
  --dump-header "$headers" \
  --output "$body" \
  "https://$host/api/ready"

jq -e '
  .status == "ready"
  and (.dependencies | type == "object" and length > 0)
  and all(.dependencies[]; . == "ready")
' "$body" >/dev/null
grep -Eiq '^cache-control:[[:space:]]*no-store([[:space:]]|$)' "$headers" || {
  echo "public readiness response is missing Cache-Control: no-store" >&2
  exit 1
}

admin_status=$(curl --silent --show-error \
  --proto '=https' \
  --tlsv1.2 \
  --connect-timeout 10 \
  --max-time 30 \
  --output /dev/null \
  --write-out '%{http_code}' \
  "https://$host/api/v1/admin/users")
[ "$admin_status" = "404" ] || {
  echo "public administrative API returned HTTP $admin_status instead of 404" >&2
  exit 1
}

echo "public health PASS: host=$host tls=verified readiness=ready admin_api=hidden"

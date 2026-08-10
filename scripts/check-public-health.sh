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

# One successful request cannot qualify an intermittently unreachable edge or
# application tier. Exercise both the Nginx-local route and the API readiness
# proxy repeatedly, rejecting Railway's five-second upstream dial/retry path
# even when a second replica eventually answers.
probe_count=${STRAYLIGHT_PUBLIC_HEALTH_PROBES:-24}
case "$probe_count" in
  ''|*[!0-9]*|0)
    echo "STRAYLIGHT_PUBLIC_HEALTH_PROBES must be a positive integer" >&2
    exit 64
    ;;
esac
headers="$temp_dir/headers"
body="$temp_dir/ready.json"
probe=1
while [ "$probe" -le "$probe_count" ]; do
  probe_result=$(curl --fail --silent --show-error \
    --proto '=https' \
    --tlsv1.2 \
    --connect-timeout 3 \
    --max-time 8 \
    --output /dev/null \
    --write-out '%{http_code} %{time_total}' \
    "https://$host/healthz") || {
      echo "public edge probe $probe/$probe_count failed" >&2
      exit 1
    }
  set -- $probe_result
  [ "$1" = "200" ] || {
    echo "public edge probe $probe/$probe_count returned HTTP $1" >&2
    exit 1
  }
  awk -v elapsed="$2" 'BEGIN { exit !(elapsed <= 2.0) }' || {
    echo "public edge probe $probe/$probe_count took ${2}s (budget: 2s)" >&2
    exit 1
  }

  ready_result=$(curl --fail --silent --show-error \
    --proto '=https' \
    --tlsv1.2 \
    --connect-timeout 3 \
    --max-time 8 \
    --dump-header "$headers" \
    --output "$body" \
    --write-out '%{http_code} %{time_total}' \
    "https://$host/api/ready") || {
      echo "public API readiness probe $probe/$probe_count failed" >&2
      exit 1
    }
  set -- $ready_result
  [ "$1" = "200" ] || {
    echo "public API readiness probe $probe/$probe_count returned HTTP $1" >&2
    exit 1
  }
  awk -v elapsed="$2" 'BEGIN { exit !(elapsed <= 2.0) }' || {
    echo "public API readiness probe $probe/$probe_count took ${2}s (budget: 2s)" >&2
    exit 1
  }
  jq -e '
    .status == "ready"
    and .dependencies.database == "ready"
    and .dependencies.object_store == "ready"
    and (.dependencies.embeddings == "ready" or .dependencies.embeddings == "degraded")
  ' "$body" >/dev/null
  grep -Eiq '^cache-control:[[:space:]]*no-store([[:space:]]|$)' "$headers" || {
    echo "public readiness response is missing Cache-Control: no-store" >&2
    exit 1
  }
  probe=$((probe + 1))
done

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

echo "public health PASS: host=$host edge_probes=$probe_count api_ready_probes=$probe_count budget_seconds=2 tls=verified readiness=ready admin_api=hidden"

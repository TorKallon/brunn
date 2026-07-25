#!/bin/sh
set -eu
umask 077

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
artifact_root=${1:-"$root/release-artifacts"}
trivy_image=${TRIVY_IMAGE:-aquasec/trivy:0.72.0@sha256:cffe3f5161a47a6823fbd23d985795b3ed72a4c806da4c4df16266c02accdd6f}

git -C "$root" diff --quiet
git -C "$root" diff --cached --quiet
[ -z "$(git -C "$root" ls-files --others --exclude-standard)" ] || {
  echo "release artifacts require a clean Git worktree" >&2
  exit 1
}

revision=$(git -C "$root" rev-parse HEAD)
branch=$(git -C "$root" branch --show-current)
tree=$(git -C "$root" rev-parse HEAD^{tree})
created_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
stamp=$(date -u +%Y%m%dT%H%M%SZ)
release_id="$stamp-${revision}"
work_dir="$artifact_root/.$release_id.partial"
release_dir="$artifact_root/$release_id"

[ ! -e "$work_dir" ] && [ ! -e "$release_dir" ] || {
  echo "release artifact path already exists: $release_dir" >&2
  exit 1
}
mkdir -p \
  "$work_dir/binaries" \
  "$work_dir/bundles" \
  "$work_dir/images" \
  "$work_dir/image-inspect" \
  "$work_dir/sboms" \
  "$work_dir/scans" \
  "$work_dir/vendor-scans" \
  "$work_dir/vendor-sboms" \
  "$work_dir/configuration" \
  "$work_dir/source"

cleanup() {
  status=$?
  trap - EXIT INT TERM
  if [ "$status" -ne 0 ] && [ -d "$work_dir" ]; then
    mv "$work_dir" "$release_dir"
    printf '%s\n' "release verification failed; inspect scans/ and manifest.json" \
      >"$release_dir/RELEASE-BLOCKED"
    echo "blocked release artifacts retained: $release_dir" >&2
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

echo "building immutable release images for $revision"
docker build \
  --file "$root/apps/api/Dockerfile" \
  --build-arg "STRAYLIGHT_BUILD_REVISION=$revision" \
  --tag "straylight-api:$revision" \
  "$root"
docker build \
  --file "$root/apps/web/Dockerfile" \
  --build-arg "STRAYLIGHT_BUILD_REVISION=$revision" \
  --tag "straylight-web:$revision" \
  "$root/apps/web"
docker build \
  --file "$root/apps/mcp/Dockerfile" \
  --build-arg "STRAYLIGHT_BUILD_REVISION=$revision" \
  --tag "straylight-mcp:$revision" \
  "$root/apps/mcp"
docker build \
  --file "$root/infra/postgres/Dockerfile" \
  --build-arg "STRAYLIGHT_BUILD_REVISION=$revision" \
  --tag "straylight-postgres:$revision" \
  "$root/infra/postgres"
docker build \
  --file "$root/infra/minio/Dockerfile" \
  --build-arg "STRAYLIGHT_BUILD_REVISION=$revision" \
  --tag "straylight-minio:$revision" \
  "$root/infra/minio"
docker build \
  --file "$root/infra/minio-client/Dockerfile" \
  --build-arg "STRAYLIGHT_BUILD_REVISION=$revision" \
  --tag "straylight-minio-client:$revision" \
  "$root/infra/minio-client"
docker build \
  --file "$root/infra/caddy/Dockerfile" \
  --build-arg "STRAYLIGHT_BUILD_REVISION=$revision" \
  --tag "straylight-caddy:$revision" \
  "$root/infra/caddy"

copy_from_image() {
  image=$1
  source=$2
  destination=$3
  container=$(docker create "$image")
  docker cp "$container:$source" "$destination"
  docker rm "$container" >/dev/null
}

copy_from_image \
  "straylight-api:$revision" \
  /usr/local/bin/straylight \
  "$work_dir/binaries/straylight"
copy_from_image \
  "straylight-api:$revision" \
  /usr/local/bin/carrystate \
  "$work_dir/binaries/carrystate"
copy_from_image \
  "straylight-minio:$revision" \
  /usr/local/bin/minio \
  "$work_dir/binaries/minio"
copy_from_image \
  "straylight-minio-client:$revision" \
  /usr/local/bin/mc \
  "$work_dir/binaries/mc"
copy_from_image \
  "straylight-postgres:$revision" \
  /usr/local/bin/postgres \
  "$work_dir/binaries/postgres"
copy_from_image \
  "straylight-web:$revision" \
  /usr/share/nginx/html/. \
  "$work_dir/bundles/web"
copy_from_image \
  "straylight-mcp:$revision" \
  /app/dist/. \
  "$work_dir/bundles/mcp"

failed_components=
repository_failed=false
for component in api web mcp postgres minio minio-client caddy; do
  image="straylight-$component:$revision"
  docker image inspect "$image" >"$work_dir/image-inspect/$component.json"
  docker save "$image" | gzip -9 >"$work_dir/images/$component.tar.gz"
  docker run --rm \
    --volume /var/run/docker.sock:/var/run/docker.sock \
    --volume "$work_dir/sboms:/output" \
    "$trivy_image" \
    image --format cyclonedx \
    --output "/output/$component.cdx.json" \
    "$image"
  if docker run --rm \
    --volume /var/run/docker.sock:/var/run/docker.sock \
    --volume "$work_dir/scans:/output" \
    "$trivy_image" \
    image --exit-code 1 --severity HIGH,CRITICAL --format json \
    --output "/output/$component.json" \
    "$image"; then
    printf '%s\n' PASS >"$work_dir/scans/$component.status"
  else
    printf '%s\n' FAIL >"$work_dir/scans/$component.status"
    failed_components="$failed_components $component"
  fi
done

if [ -n "${RELEASE_ENV_FILE:-}" ]; then
  release_env_file=$RELEASE_ENV_FILE
else
  release_env_file="$work_dir/.candidate.env"
  awk -F= -v revision="$revision" '
    BEGIN {
      replacement["STRAYLIGHT_RELEASE_REVISION"] = revision
      replacement["DD_VERSION"] = revision
      replacement["STRAYLIGHT_DATABASE_IMAGE"] = "straylight-postgres:" revision
      replacement["STRAYLIGHT_OBJECT_STORE_IMAGE"] = "straylight-minio:" revision
      replacement["STRAYLIGHT_OBJECT_STORE_CLIENT_IMAGE"] = "straylight-minio-client:" revision
      replacement["STRAYLIGHT_API_IMAGE"] = "straylight-api:" revision
      replacement["STRAYLIGHT_WEB_IMAGE"] = "straylight-web:" revision
      replacement["STRAYLIGHT_MCP_IMAGE"] = "straylight-mcp:" revision
      replacement["STRAYLIGHT_EDGE_IMAGE"] = "straylight-caddy:" revision
    }
    {
      key = $1
      if (key in replacement) {
        print key "=" replacement[key]
      } else {
        print
      }
    }
  ' "$root/.env.example" >"$release_env_file"
fi
[ -f "$release_env_file" ] || {
  echo "release environment file does not exist: $release_env_file" >&2
  exit 1
}
docker compose \
  --env-file "$release_env_file" \
  --file "$root/compose.yaml" \
  --file "$root/compose.production.yaml" \
  --profile '*' \
  config --format json \
  >"$work_dir/configuration/production-compose.json"
docker compose \
  --env-file "$release_env_file" \
  --file "$root/compose.yaml" \
  --file "$root/compose.production.yaml" \
  --profile '*' \
  config --hash '*' \
  >"$work_dir/configuration/production-service-hashes.txt"

vendor_list="$work_dir/.vendor-images"
docker compose \
  --env-file "$release_env_file" \
  --file "$root/compose.yaml" \
  --file "$root/compose.production.yaml" \
  --profile '*' \
  config --format json |
  jq -r '.services[].image' |
  LC_ALL=C sort -u |
  grep -v '^straylight-' >"$vendor_list"
printf '%s\n' "name	image	critical	high	fixable_high_or_critical" \
  >"$work_dir/vendor-images.tsv"
vendor_index=0
while IFS= read -r image; do
  vendor_index=$((vendor_index + 1))
  name=$(printf 'vendor-%02d' "$vendor_index")
  docker run --rm \
    --volume /var/run/docker.sock:/var/run/docker.sock \
    --volume "$work_dir/vendor-sboms:/output" \
    "$trivy_image" \
    image --format cyclonedx \
    --output "/output/$name.cdx.json" \
    "$image"
  docker run --rm \
    --volume /var/run/docker.sock:/var/run/docker.sock \
    --volume "$work_dir/vendor-scans:/output" \
    "$trivy_image" \
    image --severity HIGH,CRITICAL --format json \
    --output "/output/$name.json" \
    "$image"
  critical=$(jq \
    '[.Results[]?.Vulnerabilities[]? | select(.Severity == "CRITICAL")] | length' \
    "$work_dir/vendor-scans/$name.json")
  high=$(jq \
    '[.Results[]?.Vulnerabilities[]? | select(.Severity == "HIGH")] | length' \
    "$work_dir/vendor-scans/$name.json")
  fixable=$(jq \
    '[.Results[]?.Vulnerabilities[]? |
      select((.Severity == "HIGH" or .Severity == "CRITICAL")
        and (.FixedVersion // "") != "")] | length' \
    "$work_dir/vendor-scans/$name.json")
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "$name" "$image" "$critical" "$high" "$fixable" \
    >>"$work_dir/vendor-images.tsv"
done <"$vendor_list"
rm "$vendor_list"

source_tree="$work_dir/.source-tree"
mkdir "$source_tree"
git -C "$root" archive HEAD | tar -x -C "$source_tree"
if docker run --rm \
  --volume "$source_tree:/workspace:ro" \
  --volume "$work_dir/scans:/output" \
  "$trivy_image" \
  filesystem --exit-code 1 --severity HIGH,CRITICAL \
  --scanners vuln,secret,misconfig --format json \
  --output /output/repository.json \
  /workspace; then
  printf '%s\n' PASS >"$work_dir/scans/repository.status"
else
  printf '%s\n' FAIL >"$work_dir/scans/repository.status"
  repository_failed=true
fi
rm -rf "$source_tree"

(
  cd "$root"
  git ls-files \
    apps/api/src \
    apps/api/migrations \
    apps/api/Cargo.lock \
    native_memory.py \
    agent_work_eval.py \
    transition_eval.py \
    eval |
    LC_ALL=C sort |
    while IFS= read -r path; do
      shasum -a 256 "$path"
    done
) >"$work_dir/source/reasoning-and-evaluation.sha256"
(
  cd "$root"
  git ls-files \
    apps/api/Cargo.lock \
    apps/web/package-lock.json \
    apps/mcp/package-lock.json |
    LC_ALL=C sort |
    while IFS= read -r path; do
      shasum -a 256 "$path"
    done
) >"$work_dir/source/dependency-locks.sha256"

reasoning_fingerprint=$(shasum -a 256 \
  "$work_dir/source/reasoning-and-evaluation.sha256" | awk '{print $1}')
dependency_fingerprint=$(shasum -a 256 \
  "$work_dir/source/dependency-locks.sha256" | awk '{print $1}')

jq -n \
  --arg format "straylight-release@v1" \
  --arg release_id "$release_id" \
  --arg created_at "$created_at" \
  --arg revision "$revision" \
  --arg branch "$branch" \
  --arg tree "$tree" \
  --arg reasoning_fingerprint "$reasoning_fingerprint" \
  --arg dependency_fingerprint "$dependency_fingerprint" \
  --arg failed_components "$(printf '%s' "$failed_components" | xargs)" \
  --argjson repository_failed "$repository_failed" \
  '{
    format: $format,
    release_id: $release_id,
    created_at: $created_at,
    git: {
      revision: $revision,
      branch: $branch,
      tree: $tree,
      clean: true
    },
    fingerprints: {
      reasoning_and_evaluation: $reasoning_fingerprint,
      dependency_locks: $dependency_fingerprint,
      production_compose: "configuration/production-compose.json",
      production_service_hashes: "configuration/production-service-hashes.txt"
    },
    vulnerability_gate: {
      severity: ["HIGH", "CRITICAL"],
      repository_passed: ($repository_failed | not),
      failed_components: (
        if $failed_components == "" then [] else $failed_components | split(" ") end
      )
    },
    vendor_runtime_inventory: "vendor-images.tsv"
  }' >"$work_dir/manifest.json"

(
  cd "$work_dir"
  find . -type f ! -name CHECKSUMS.sha256 -print |
    LC_ALL=C sort |
    while IFS= read -r path; do
      shasum -a 256 "$path"
    done >CHECKSUMS.sha256
)

if [ -n "$failed_components" ] || [ "$repository_failed" = true ]; then
  echo "release blocked by repository or image vulnerability gate:$failed_components" >&2
  exit 1
fi

mv "$work_dir" "$release_dir"
trap - EXIT INT TERM
echo "release artifacts complete: $release_dir"

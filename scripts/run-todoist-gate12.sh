#!/usr/bin/env bash
set -u

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
artifact_dir=${STRAYLIGHT_TODOIST_GATE12_ARTIFACT_DIR:-"$repo_root/release-artifacts/task-gate12/todoist"}
test_database_url=${STRAYLIGHT_TEST_DATABASE_URL:-}

if [ -z "$test_database_url" ]; then
  echo "STRAYLIGHT_TEST_DATABASE_URL must name a disposable migrated database" >&2
  exit 2
fi

mkdir -p "$artifact_dir"
started_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
overall=pass

run_check() {
  check_name=$1
  shift
  log_path="$artifact_dir/$check_name.log"
  start_seconds=$(date +%s)
  (
    cd "$repo_root"
    "$@"
  ) >"$log_path" 2>&1
  exit_code=$?
  end_seconds=$(date +%s)
  printf '%s\t%s\t%s\n' "$check_name" "$exit_code" "$((end_seconds-start_seconds))" \
    >>"$artifact_dir/checks.tsv"
  if [ "$exit_code" -ne 0 ]; then
    overall=fail
  fi
}

: >"$artifact_dir/checks.tsv"
run_check fixture_feature_build \
  /Users/aether/.cargo/bin/cargo build --locked --manifest-path apps/api/Cargo.toml \
  --features todoist-fixture --bins
run_check real_stack_owner_web_fixture \
  python3 tests/live_todoist_contract.py --artifact-dir "$artifact_dir"
run_check rust_fixture_scenario \
  /Users/aether/.cargo/bin/cargo test --locked --manifest-path apps/api/Cargo.toml \
  --test todoist_sync_database -- --nocapture --test-threads=1
run_check rust_rls_isolation \
  /Users/aether/.cargo/bin/cargo test --locked --manifest-path apps/api/Cargo.toml \
  --test todoist_rls_database -- --nocapture
run_check rust_mapping_and_scheduler \
  /Users/aether/.cargo/bin/cargo test --locked --manifest-path apps/api/Cargo.toml \
  --lib todoist_sync::tests -- --nocapture
run_check static_no_mutation \
  python3 -m unittest tests.test_todoist_read_only_contract -v
run_check api_and_migration_contract \
  python3 -m unittest tests.test_todoist_api_routes tests.test_todoist_migration_contract -v
# A synthetic token canary is intentionally created inside the database test.
# It may exist in test source, but it must never reach recorded runtime output.
if grep -R -F -q -- 'todoist-secret-canary-7f12' "$artifact_dir"/*.log; then
  printf '%s\t%s\t%s\n' secret_canary_runtime_scan 1 0 >>"$artifact_dir/checks.tsv"
  overall=fail
else
  printf '%s\t%s\t%s\n' secret_canary_runtime_scan 0 0 >>"$artifact_dir/checks.tsv"
fi

finished_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
revision=$(git -C "$repo_root" rev-parse HEAD)
export TODOIST_GATE12_STATUS=$overall
export TODOIST_GATE12_STARTED=$started_at
export TODOIST_GATE12_FINISHED=$finished_at
export TODOIST_GATE12_REVISION=$revision
export TODOIST_GATE12_ARTIFACT_DIR=$artifact_dir
python3 - <<'PY'
import hashlib
import json
import os
from pathlib import Path

artifact_dir = Path(os.environ["TODOIST_GATE12_ARTIFACT_DIR"])
checks = []
for line in (artifact_dir / "checks.tsv").read_text(encoding="utf-8").splitlines():
    name, raw_exit, raw_seconds = line.split("\t")
    log = artifact_dir / f"{name}.log"
    check = {
        "name": name,
        "status": "pass" if raw_exit == "0" else "fail",
        "exit_code": int(raw_exit),
        "elapsed_seconds": int(raw_seconds),
    }
    if log.is_file():
        check["output"] = log.name
        check["output_sha256"] = hashlib.sha256(log.read_bytes()).hexdigest()
    checks.append(check)

repo_root = artifact_dir.parents[2]
fixture_root = repo_root / "apps/api/tests/fixtures/todoist/v1"
fixtures = {}
for path in sorted(fixture_root.glob("*.json")):
    fixtures[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()

evidence = {
    "schema": "straylight-todoist-gate12e@v1",
    "status": os.environ["TODOIST_GATE12_STATUS"],
    "started_at": os.environ["TODOIST_GATE12_STARTED"],
    "finished_at": os.environ["TODOIST_GATE12_FINISHED"],
    "revision": os.environ["TODOIST_GATE12_REVISION"],
    "fixture_sha256": fixtures,
    "checks": checks,
}
(artifact_dir / "scenario.json").write_text(
    json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
print(json.dumps(evidence, indent=2, sort_keys=True))
PY

if [ "$overall" != pass ]; then
  exit 1
fi

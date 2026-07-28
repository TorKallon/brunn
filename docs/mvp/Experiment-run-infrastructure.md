# Shared experiment run infrastructure

Status: Implemented in harnesses; product-specific flags still require their
own Dxx implementation and status exposure

## Isolated Nyx execution preamble

Every experiment runs from a clean detached worktree at one immutable full
revision. One coordinator builds the shared local image once; agents must not
rebuild the static `straylight-api:local` tag concurrently. Every subsequent
Compose command uses `--no-build`, a unique project name, and a private env
file/port allocation:

```bash
FREEZE="$(git rev-parse --show-toplevel)"
REV="$(git rev-parse HEAD)"
PROJECT="slx-eNN-arm"
ENV_FILE="$FREEZE/runs/private/$PROJECT.env"
COMPOSE=(docker compose --project-name "$PROJECT" --env-file "$ENV_FILE" -f "$FREEZE/compose.yaml")

DD_VERSION="$REV" "${COMPOSE[@]}" build api migrate worker
"${COMPOSE[@]}" up -d --no-build api
API_CONTAINER="$("${COMPOSE[@]}" ps -q api)"
DB_CONTAINER="$("${COMPOSE[@]}" ps -q db)"
test -n "$API_CONTAINER"
test -n "$DB_CONTAINER"
```

Only the coordinator runs the build line. Start `worker` only for an arm that
must backfill real or mock semantic embeddings. Never run bare
`docker compose`: `compose.yaml` has a static project name and an unscoped
command can target the owner stack. Performance runs are exclusive on Nyx;
reasoning draws may use separate project/port/volume lanes after deterministic
gates pass.

Every definitive performance command supplies:

```text
--protocol simple
--api-container "$API_CONTAINER"
--db-container "$DB_CONTAINER"
--expect-build-revision "$REV"
```

It also declares every arm-specific boolean with
`--expect-feature-flag`, every typed knob with `--expect-runtime-config`, and
one semantic-failure posture:

- `required`, with `--wait-semantic` plus distinct provider-failure and restore
  hooks, for any semantic-enabled arm;
- `not-applicable` only for a service whose authenticated runtime has
  `semantic_lane=false`, whose requested modes exclude semantic, and which is
  not waiting for semantic readiness.

The default query-count contract is runtime-bound to the default-safe query
shape. Any non-default query shape uses a named
`--query-budget-profile` and an explicit `--query-budget-contract`. A launch
profile never inherits `eval/query_budgets.json`; absence of the calibrated
launch contract is a preflight failure.

Use `--query-budget-profile calibration` only to capture counts for a new
shape. A calibration run records all measurements but contains an intentionally
failing `query_budget_calibration_is_not_acceptance` gate and can never serve as
acceptance evidence. Review its counts, author a contract whose `profile` and
`runtime_features` bind the intended shape, then rerun with that named profile
and contract path.

## Separate-arm identity

When multiple arms use the same harness condition in separate invocations,
every invocation must supply both:

```text
--experiment-arm <stable-arm-name>
--paired-draw-id <shared-experiment-draw-name>
```

`--experiment-arm` is the statistic's arm identity. `--paired-draw-id` is
identical for every arm in one paired draw, while `--run-id` remains unique to
the individual invocation. The harness writes this identity into an immutable
run-directory binding and the run ledger. Resume with a different identity,
condition, case set, model, protocol, or manifest fingerprint fails closed.

Do not use these options for E01/E10-style same-invocation comparisons: those
runs already have distinct condition names, and the aggregator preserves the
condition-as-arm behavior.

## Runtime snapshot

Every service-backed run makes an authenticated `/v1/status` request and stores
`service_runtime_snapshot`, including build revision, every exposed feature
flag/knob, embedding posture, and any semantic runtime counters. The canonical
snapshot hash is bound into `run_ledger.artifacts`.

Use `--expect-feature-flag NAME=on|off` for booleans,
`--expect-runtime-config NAME=<JSON>` for typed knobs, and
`--expect-build-revision REVISION` when pinning an isolated image. Any missing
field or expected/actual mismatch aborts before reasoning.

Native operation accounting keeps pre-HTTP CLI construction failures separate
from measured service work. A definitive record may retain an
`http_status=0` `failed:X` under `local_cli_failures` only when a later
successful HTTP operation named `X` appears in the same record; the validator
recomputes that summary and all service totals. These local failures and their
characters remain diagnostic and stay in `model_visible_tool_output_chars`,
but are excluded from service call/HTTP-call, result/source/metadata/replay
character, and latency metrics. Any measured 4xx or `denied:*` operation, or
any unrecovered local failure, invalidates definitive evidence. Agent-work
service prompts provide the canonical shell-safe positional-JSON checkpoint
invocation rather than leaving the wrapper syntax implicit.

## Aggregation

The aggregator infers a complete arm set per suite and rejects an incomplete
arm for any suite/draw/case, changing case sets between draws, duplicate
observations, fewer than three complete draws, mixed explicit/condition
identity modes, arm-to-condition drift, runtime-feature drift within an arm,
or mixed source/grader/manifest/build fingerprints. Declare the intended
ordering explicitly for feature experiments:

```bash
python3 eval/aggregate_draws.py results/...json \
  --expected-arm treatment \
  --expected-arm control \
  --expected-arm-retrieval-modes treatment=exact,lexical \
  --expected-arm-retrieval-modes control=exact,lexical \
  --out results/...-aggregate.json
```

For a definitive aggregate, repeat `--expected-arm-retrieval-modes` exactly
once for every service-backed arm and omit filesystem-only arms. The saved
top-level, experiment-binding, and ledger modes must agree with that mapping;
all service artifacts must also share one source revision, runtime build
revision, Docker image ID, and image revision. Separate container instances
are allowed.

One narrow exception exists for a predeclared longitudinal extension such as
E04. Supply both
`--case-extension-plan eval/e04_case_extension_plan.json` and its frozen
`--case-extension-plan-sha256
3cf08c940c527d2eb309b2263a4ad2303b3c8aeede1bc7ce15076b6670373976`.
The aggregator verifies the plan file hash, each exact parent-manifest hash and
case list, the strict extension subset, and the declared base/extension draw
counts. Every included case must still have at least three complete draws, each
case/draw must contain the full arm set, and all artifacts for a suite must
retain the parent manifest fingerprint. Use the parent manifest with repeated
`--case` selectors for extra draws; do not substitute a separately
fingerprinted subset manifest into the aggregate. A modified plan requires a
new explicit owner-reviewed hash; computing the expected hash from the same
modified file is not a predeclaration.

Default McNemar output remains two-sided on majority-collapsed case outcomes.
E06 additionally requests its directional claim-level statistic with:

```text
--claim-mcnemar-alternative a_greater
```

Here arm A must be the treatment because `--expected-arm` order is
load-bearing. Claim outcomes are paired by suite, case, and claim ID, then
strict-majority collapsed across draws before the exact one-sided test.

## Accepted-source context audit

E04's checker reads the saved `service_operations[].source_paths`; answer
citations do not count as returned context. It emits
`straylight-accepted-source-context-audit@v1`:

```bash
python3 eval/audit_accepted_sources.py \
  results/2026-MM-DD-e04-A-*-draw*.json \
  results/2026-MM-DD-e04-B-*-draw*.json \
  results/2026-MM-DD-e04-C-*-draw*.json \
  --expected-arm-retrieval-modes e04-A=exact,lexical \
  --expected-arm-retrieval-modes e04-B=exact,lexical \
  --expected-arm-retrieval-modes e04-C=exact,lexical \
  --out results/2026-MM-DD-e04-accepted-source-context.json
```

The artifact reports all-claim and missed-claim rates overall, by arm, and by
suite, plus a per-claim evidence table. Missing run provenance, structured
grades, operation receipts, or source-path metrics fails closed. Filesystem
artifacts, aggregates, prior audits, and performance soaks are deliberately
excluded.

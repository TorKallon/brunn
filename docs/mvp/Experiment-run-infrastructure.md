# Shared experiment run infrastructure

Status: Implemented in harnesses; product-specific flags still require their
own Dxx implementation and status exposure

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
  --out results/...-aggregate.json
```

One narrow exception exists for a predeclared longitudinal extension such as
E04: `--allow-case-extension` permits a frozen subset to receive extra paired
draws. Every included case must still have at least three complete draws, each
case/draw must contain the full arm set, and all artifacts must retain one
manifest fingerprint. Use the parent manifest with repeated `--case` selectors
for the extra draws; do not substitute a separately fingerprinted subset
manifest into the aggregate.

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
python3 eval/audit_accepted_sources.py results/2026-MM-DD-e04-*.json \
  --out results/2026-MM-DD-e04-accepted-source-context.json
```

The artifact reports all-claim and missed-claim rates overall, by arm, and by
suite, plus a per-claim evidence table. Missing run provenance, structured
grades, operation receipts, or source-path metrics fails closed.

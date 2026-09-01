# Checkpoint query-count diagnostic

Status: diagnostic harness ready; no live run performed

This diagnostic identifies the extra SQLx statement behind a checkpoint
`query_count` difference without changing `eval/query_budgets.json`.
`eval/checkpoint_query_diagnostic.py`:

- provisions one case-isolated simple-protocol Markdown document;
- submits at least 30 fresh, committed checkpoint requests with one inferred
  source and no explicit `source_refs`;
- records each response's `query_count` and exact `x-request-id`;
- requires unique request IDs and checkpoint identities;
- binds the run to the diagnostic script SHA-256, Git revision/tree, and a
  clean tracked harness checkout;
- atomically removes the evaluation user and verifies that its credential is
  revoked before the artifact can pass; and
- correlates only those request IDs to normalized SQLx JSON log events.

The run artifact never records either the admin or scoped credential. The raw
API log remains private operational evidence and should live under an ignored
owner-only `runs/.../private/` path.

## Runtime preparation

The API process must be created with normal SQLx statements visible to the JSON
formatter:

```text
RUST_LOG=info,brunn=debug,sqlx::query=debug
```

Setting this after process start is insufficient. SQLx logs parameterized SQL,
not bound values. The correlation artifact nevertheless copies only matching
request events and records that bindings are absent.

Use the Mode 1 stack for `--runtime-posture mode1-pending`. Use the owned Mode 2
mock stack for `--runtime-posture semantic-ready`; do not point this diagnostic
at a billable provider merely to explain a query-count discrepancy.

## Run and correlate

With `BRUNN_API_URL` and `BRUNN_EVAL_TOKEN` already loaded from a
private environment file:

```bash
umask 077
REV=4cde5edb809cb4158d632256273b5b611db8728a
RUN_DIR=runs/e03-query-audit
mkdir -p "$RUN_DIR/private"
python3 eval/checkpoint_query_diagnostic.py run \
  --run-id e03-checkpoint-query-semantic-ready \
  --runtime-posture semantic-ready \
  --expect-build-revision "$REV" \
  --samples 30 \
  --out "$RUN_DIR/semantic-ready-run.json"
docker logs "$API_CONTAINER" >"$RUN_DIR/private/api-sqlx.jsonl"
python3 eval/checkpoint_query_diagnostic.py correlate \
  --artifact "$RUN_DIR/semantic-ready-run.json" \
  --api-log "$RUN_DIR/private/api-sqlx.jsonl" \
  --out "$RUN_DIR/semantic-ready-sql.json"
```

Repeat against Mode 1 with a distinct run ID/output and
`--runtime-posture mode1-pending`. Do not reuse an evaluation identity or output
path: both commands refuse incomplete evidence, and artifact writes refuse
overwrite.

The correlation passes only when every request's SQLx event count exactly
equals the `query_count` returned in that response. `sequence_groups` contains
the ordered normalized SQL and SHA-256 fingerprint once per distinct request
shape. It binds the exact run-artifact file bytes, rather than a reserialized
JSON object. Compare the dominant Mode 1 and semantic-ready sequences to name
the inserted statement.

`COMMIT` and `ROLLBACK` are retained if SQLx emits corresponding
`sqlx::query` events. SQLx 0.9 queues the rollback for a dropped transaction via
`queue_simple_query`, which does not itself create a `QueryLogger`; therefore,
the absence of a standalone `ROLLBACK` event does not prove that PostgreSQL
received no rollback. It does prove that rollback is outside the current
`query_count` event definition.

## Decision rule

- If the 29th event is a normalized-path fallback or another avoidable
  application query, fix the request path and retain the global `<=28`
  checkpoint budget.
- If repeated Mode 1 and semantic-ready traces prove one required, explicitly
  identified conditional statement, use one global upper bound that covers the
  legitimate shape. Do not create an arm-specific budget unless product source
  actually contains an arm-specific checkpoint branch.
- A scalar `28`/`29` artifact without correlated statements is insufficient to
  raise the budget.

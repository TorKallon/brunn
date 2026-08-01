# D12 — Operational Simplification

Status: Railway topology, import, clients, backfill, and final worker passed; repository publication pending
Date: 2026-07-31
Depends on: D08 (legacy lifecycle), D14 (migration and authority cutover)
Gated by: deterministic production and recovery checks below
Runtime flag: `embedding_backfill_guard` for the backfill throttle

## Current state

The intended single-hosted-target design is now real: Railway runs the public
web edge, private simplified API, separate worker, and PostgreSQL; production
objects use external versioned S3. Nyx is the operator, build, test, and future
restore-rehearsal host. It does not accept production writes and is not an
owner pilot.

The API is healthy and ready at build
`39761166d21b0cfa44d11e3ba18a52112693d0cd`, with all 56 migrations applied.
Context-shaping treatments and dreaming are off; operational cache, guard, and
timing features are on. The layered import and zero-diff audits pass. At the
pre-worker snapshot, 12,727 jobs were queued with zero running/complete/failed.
Web deployment `316d90eb-d807-4091-84d4-8ba10b49a2f2` passes. All 12,727 jobs
completed at zero queued, running, or failed; 126,536 search chunks have zero
missing embeddings. Permanent worker deployment
`7af78da7-3b01-4a66-9923-3aa8184d1978` is `SUCCESS` with exactly one running
replica and prior deployments removed. The isolated restore attempt was
environment-blocked on locked Nyx and is non-blocking for this direct cutover.

## Architecture

**S3-only production object store.** Production uses an external versioned S3
store; MinIO remains dev/test-only. Binary verification stays SHA-256-based and
store-agnostic. The pre-cutover object set is retained alongside a checksummed
PostgreSQL dump until final acceptance.

**One production: Railway.** The SPA is the public edge. The API, worker, and
database remain private. MCP is a local stdio process pinned to an immutable
bundle and targets the hosted `/api` edge; it is not a hosted service. Nyx is
not a second authority.

**Worker isolation is structural.** The worker owns embedding backfill and
future asynchronous accelerators. The API must serve complete exact+lexical
behavior while the worker is stopped. This directly addresses the earlier
v5/v6 index-catchup contention and the 2026-07-26 unbudgeted-bookkeeping
collapse.

**Foreground-latency backfill guard.** The API publishes only content-free
rolling open/search latency counts, p95s, ages, and snapshot time. The worker
fails closed when a configured snapshot is missing, invalid, stale, from the
future, or above the configured limits. `embedding_backfill_guard=false`
remains the immediate halt.

**Production storage is sized, with efficiency follow-up.** Railway Pro is
active and its confirmed $20/month minimum is infrastructure spend, not
embeddings. The database volume is 20 GB in both live state and checked-in IaC;
the final filesystem is 25% used with 13.6 GiB free. The simplified and legacy
HNSW indexes have each recorded zero scans since July 25 because their semantic
and legacy routes are off. They are distinct derived/rebuildable accelerators,
not authoritative source/history. Neither was dropped; audit storage efficiency
separately, and retain `corpus_members` until restore-backed legacy retirement.
`VACUUM ANALYZE` on `search_chunks` and `jobs` completed successfully.

**Monitoring stays narrow.** The target set remains API availability, write
p95, queue age, and backup success. Synthetic monitor qualification is deferred
outside this owner-cutover completion set; do not describe it as deployed
evidence until it exists.

## Current cutover controls

1. **Passed:** hold worker execution through history replay, checkpoint import,
   fresh overlay, and the zero-diff full export audit.
2. **Passed:** restore the ordinary 600/minute limit after bounded import.
3. **Passed:** disable legacy/evaluation APIs, remove wrong variable names, and
   verify all three disabled probes return 404.
4. **Passed:** start worker processing only after the passed audits and observe
   it under the foreground guard. The guard paused correctly 118 times during
   an intentionally slow broad-open sample after the queue was already zero;
   the final worker emitted no `53100`, error, fatal, or job failure.
5. Use ChatGPT-authenticated Codex for reasoning. API-key billing is limited to
   embeddings; the conservative current upper-bound estimate is $3.61, below
   the $20 notification threshold. Actual provider billing is unavailable.

## Historical evidence retained

- The MinIO production candidate had critical/high CVEs; removing it from the
  production architecture avoids carrying that remediation tax.
- The 640K exact+lexical baseline measured open p95 59.7 ms and search p95
  53.1 ms.
- Earlier v5/v7 writes regressed above three seconds, and v5/v6 showed index
  catchup contending with foreground reads. Those results motivate separate
  worker resources, guarded backfill, and production p95 observation.

## Acceptance gates

1. API, web, and worker report the intended final revision; health/readiness and
   public-route isolation pass. **Passed; the permanent worker is `SUCCESS` at
   one replica and prior deployments are removed.**
2. With worker execution held out, exact+lexical fidelity and production client
   canaries pass. **Passed for import, Codex, and Aether/OpenClaw.**
3. With the worker running, the guard demonstrably pauses/resumes within one
   batch and foreground open/search p95 stays within the configured limits.
   **Passed: 30 open + 30 exact-search samples, zero failures; p95 31.809529 ms
   and 29.295206 ms against 120 ms and 107 ms limits.**
4. PostgreSQL plus S3 restore to an isolated target reproduces current paths,
   full history, hashes, binary-description receipts, native records, and
   checkpoints with zero differences. **Not performed: locked Nyx blocked
   Docker before a restore container was created. This is retained as future
   operational evidence and does not block the direct owner cutover.**
5. Each of the four monitors is qualified with content-free evidence. **Deferred
   outside the owner cutover.**
6. The ordinary request budget and evaluation API state are restored before
   client handoff. **Passed.**

## Rollback

Stop the worker first, then roll API/web to their retained pre-cutover images
and restore from the checksummed PostgreSQL dump plus versioned S3 if the
database itself is invalid. Do not point production clients at Nyx. The exact
fresh source snapshot is recovery evidence, not a second writable authority.

## References

- [D08 legacy freeze and deletion](D08-legacy-freeze-and-deletion.md)
- [D14 migration and authority cutover](D14-migration-and-authority-tiers.md)
- [D11 semantic lane policy](D11-semantic-lane-policy.md)
- [2026-07-31 aggregate cutover record](../../results/2026-07-31-railway-simplified-cutover.md)
- `results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json`

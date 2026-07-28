# D10 — Read-Path Round-Trip Reductions

Status: Implemented behind default-off flags — deterministic qualification pending
Date: 2026-07-27
Depends on: D09 (per-operation query-count budget assertions; D09-latency-contract-and-gates.md) — must land first so each win is locked by an assertion the day it ships
Gated by: none for the safe subset below; the deferred lexical-scan consolidation is gated by E05 (E05-lexical-consolidation-guard.md)
Runtime flag: `read_path_roundtrip_v1` (kill switch reverts every item to the current sequential query paths without a deploy)

## Implementation note (2026-07-27)

The default-off implementation now covers the safe subset: generation is
piggybacked onto the common open/search/read/changes statements (with a
correctness fallback for read/search requests that execute no eligible primary
statement), hydration size and content are fetched together, checkpoint/change
work overlaps retrieval dispatch, checkpoint sources resolve in one batched
lookup, and the advisory lock gets one server-bounded 250ms wait. Flag off
retains the prior response and sequential-query behavior. The separately
default-off `lexical_single_scan` treatment was evaluated by E05 and rejected;
it remains off permanently.

This is implementation readiness, not acceptance evidence. D09's
request-scoped `query_count` counter and checked-in fail-closed budgets are now
implemented, and the D10 generation-piggyback lexical wrapper shares its SQL
constant with the D09 drift contract. Acceptance gate 1 and the query-count
portion of gate 3 still require the coordinated isolated-stack run: record both
safe-subset flag states, confirm the default-safe budget, and pin the lower
treatment count only from the resulting artifact. No substitute query-count
claim is recorded. E05 separately measured 795 paired
`lexical_single_scan` search samples and observed zero reductions.

## Problem and evidence

The read path spends its time on round-trips, not on data. On the clean 3,340-record fixture (results/2026-07-27-3340-clean-30-sample.json), open p95 is 26.278ms while a single exact read is 0.067ms — roughly two orders of magnitude between one query and the composed operation. At 640K records (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json) open p95 is 59.7ms, search 53.1ms, checkpoint 17.1ms. Specific waste, all in `apps/api/src/simple_core.rs`:

1. `current_generation` is fetched with a separate `SELECT max(generation)` transaction on every request, then the main query runs.
2. Hydration issues one query for sizes and a second for content.
3. `open` runs ~7–9 sequential awaits (checkpoint → changes → retrieval lanes) even though the lanes themselves already run concurrently under RETRIEVAL_LANE_TIMEOUT (~2.5s).
4. Checkpoint source resolution performs up to 64 individual path lookups.
5. Advisory-lock contention returns 409 immediately, pushing retry cost onto clients.

None of this threatens today's hard gates (open ≤5,000ms etc. — 50–100x looser than measured). The reason to do it now is budget hygiene before Tier B/C scale (D14-migration-and-authority-tiers.md): per-operation round-trip budgets are a hard constraint, and every query we remove is a query D09's assertions can pin so it never comes back.

## Design

Safe subset — no retrieval-semantics change, byte-identical responses, no experiment needed:

1. **Piggyback `current_generation` into the main query.** Fold `max(generation)` into the primary statement (subselect or CTE), eliminating one transaction per request across open/query/read/changes.
2. **Merge hydration size+content queries** into a single statement returning both.
3. **Parallelize open's sequential awaits.** Checkpoint load, changes-since-checkpoint (≤200 rows), and lane dispatch have no data dependencies between them; join them concurrently. Lane internals are untouched.
4. **Batch checkpoint source resolution.** The ≤64 exact source refs ("path | version N | sha256:...") resolve via one `WHERE path = ANY($paths)` query instead of per-path lookups.
5. **Bounded advisory-lock wait.** Wait up to 250ms for the lock before returning 409. 250ms is an order of magnitude above concurrent-write p95 (29.0ms, v8 soak) yet invisible against the 2,000ms checkpoint gate. The wait is a hard bound, never a queue.

**REJECTED by E05 (E05-lexical-consolidation-guard.md):** consolidating the
up-to-3 sequential lexical candidate scans into one. Both 640K soaks passed,
but all 795 paired search query-count deltas were zero, so the blocking strict
improvement gate failed before reasoning. Drop `lexical_single_scan`
permanently. This is also a retrieval-semantics change class: the v6
recent-first lexical change looked like a harmless efficiency win and
collapsed Star Rupture to 0/3 by hiding older authoritative sources. Migration
0055's rule — "a sparse recent match is only a lead" — exists because of that
failure.

**LEAVE ALONE:** chunk delete/reinsert on edit. Write p95 is 17.1ms (checkpoint) / 29.0ms (concurrent write) in the v8 soak, and the write path regressed twice in one day when touched (v5 unrelated-write p95 3,404ms, v7 3,170ms, per the v5/v7 future-soak JSONs) — and only the 640K soak caught it. There is no problem here to solve.

## What this does NOT change

- No change to scoring (exact 10.0 flat, lexical 3.0+ts_rank_cd capped 8.0, semantic 2.0+(1−distance)), lane structure, or the recent-first two-tier window.
- No change to any cap or budget: 128 candidates, 96,000 excerpt chars, 2,400/excerpt, ≤3 sections/entry, first-query-first budget order, open's ≤32 candidates / ≤8 hydrated docs / 24,000-char complete source.
- No schema change; no new tables or indexes; no caching layer (in-process caches remain allowed by the hard constraints but are not part of this design).
- Context-bearing response fields are byte-identical for identical inputs.
  D09's diagnostic `timings_ms` metadata is expected to reflect the changed
  execution schedule and is excluded from that comparison.
- Semantic lane behavior unchanged: still skipped with `semantic_unavailable` until indexed; the synchronous uncached query-embedding call (simple_core.rs:3005) is out of scope here.

## Failure-mode analysis

- **v6 recent-first collapse:** the direct ancestor of the rejected
  `lexical_single_scan` item; E05 killed that item and everything else avoids
  candidate-selection logic entirely.
- **2026-07-22 dedup revert:** context reduction disguised as cleanup. The safe subset reduces queries, not context — enforced by the byte-identical response gate.
- **07-26 bookkeeping collapse:** unbudgeted synchronous work. The 250ms lock wait is the only added latency anywhere, and it is bounded and asserted; parallelizing awaits removes wall-clock time without adding work.
- **Write-path regressions (v5/v7):** the write path is explicitly out of scope; the per-release soak still runs the concurrent write/search probe to catch accidental coupling.
- **Overfetch (~70,814 RuptureOps chars/case):** unaffected — this design changes how many queries fetch the context, not how much context is fetched.

## Acceptance gates

Deterministic (all must pass before enabling the flag by default):

1. D09 budget assertions updated to the new counts (e.g., open loses the generation transaction and 63 source lookups) and failing-on-regression in CI.
2. Byte-identical context diff: fixed corpus, replay identical requests
   flag-on vs flag-off, zero diffs after excluding D09 diagnostic timing
   metadata.
3. `python performance_eval.py run --label read-path-roundtrip-v1 --future-soak --out results/2026-MM-DD-read-path-roundtrip-v1-soak.json`, 30-sample definitive, scales 1k/10k/64k plus 640k: all p95s within the D09 regression-tier gates (D09-latency-contract-and-gates.md — open ≤500ms, search ≤500ms, exact read ≤100ms, checkpoint ≤200ms, resume ≤400ms, concurrent write ≤500ms / search ≤750ms) and showing no regression beyond run-to-run noise against the v8 baselines (open 59.7ms, search 53.1ms, broad 54.8ms, exact read 16.2ms, checkpoint 17.1ms, resume 35.2ms; concurrent write 29.0ms / search 100.9ms — reference points, not exact ceilings; the measured values are noise-level and an exact at-or-below gate would flake), correctness markers green, GIN idx_scan deltas via pg_stat_user_indexes unchanged, clean-git-tree fingerprint.
4. Lock-wait test: contended writer observes ≤250ms added latency then 409; uncontended path adds zero.

No reasoning experiment is required for the safe subset because responses are
byte-identical. E05 completed negative and permanently closes the separate
lexical consolidation item.

## Rollout and kill switch

Ship all five safe-subset items behind `read_path_roundtrip_v1`, default off.
Enable in dev, then on Nyx canaries, then default-on after one clean soak. The
kill switch restores current behavior at runtime with no deploy. Do not ship
the rejected `lexical_single_scan` flag; it remains off and is never bundled
into the safe subset.

## References

- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json; results/2026-07-27-3340-clean-30-sample.json; v5/v7 future-soak JSONs (write-path regressions).
- Migration 0055 (bounded full-index fallback rationale); vault note on the v6 recent-first negative experiment; vault note on the 2026-07-22 dedup revert.
- D09-latency-contract-and-gates.md; E05-lexical-consolidation-guard.md; D14-migration-and-authority-tiers.md.

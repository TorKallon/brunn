# D09 — Latency Contract and Gates

Status: Implemented in harness — isolated-stack 64K/640K acceptance runs remain
Date: 2026-07-27
Depends on: none
Gated by: none (not context-shaping; all gates here are deterministic, no paired-draw experiment required)
Runtime flag: `observability.timings_ms` (default on; kill switch strips the field from responses, nothing else changes)

## Problem and evidence

The hard SLO gates (open p95 ≤5,000ms, search ≤3,000ms, read ≤1,000ms, checkpoint ≤2,000ms) are 50-100x looser than measured behavior. The v8 640K-record soak (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json) measured: open p95 59.7ms, search 53.1ms, broad 54.8ms, exact read 16.2ms, checkpoint 17.1ms, resume 35.2ms, concurrent write 29.0ms, concurrent search 100.9ms. A gate that only fires at 5,000ms lets a 30x regression ship silently.

That is not hypothetical. Write-path latency regressed twice in one day — v5 unrelated-write p95 3,404ms, v7 3,170ms (per the v5/v7 future-soak JSONs) — and only the 640K soak caught it. The 2026-07-26 production collapse came from unbudgeted synchronous bookkeeping that no test gated: work that is cheap per-call at small scale and invisible until the corpus grows. We also have no per-phase attribution: when search p95 moves, we cannot say whether embed, lexical, HNSW, merge, or budget accounting moved it. E03 (E03-semantic-ready-latency-profile.md) is blocked on exactly that attribution.

## Design

Four parts. (a) is a measurement enabler; (b)-(d) are CI gates.

### (a) `timings_ms` per-phase decomposition (response field, NOT schema)

Every `/v1/workspace/*` response gains an optional top-level `timings_ms` object, populated server-side from monotonic clocks, emitted as response metadata only. No table, no column, no migration — this is in-process instrumentation serialized at the edge, consistent with the no-schema-expansion constraint.

- Open: `checkpoint_read`, `changes`, `lanes` (with per-lane children), `hydrate`, `generation`, `total`.
- Search: per-lane `embed`, `exact`, `lexical`, `semantic`, `merge`, `budget`, `total`.

The mutually exclusive top-level wall phases must sum to within 5% of `total`
(`unattributed` is itself a signal). Retrieval-lane child timings are
diagnostics: exact and lexical execute concurrently, so their durations must
not be added to each other or to `retrieval_wall`. The field is excluded from
checkpoint text, excerpts, and anything an agent can quote back into context;
MCP clients pass it through untouched. `performance_eval.py` records
`timings_ms` per sample so p50/p95/p99 per phase becomes reportable.

Implementation note (2026-07-27): part (a) emits open/search phase timing
metadata behind `STRAYLIGHT_OBSERVABILITY_TIMINGS_MS` (default on), records
phase percentiles in `performance_eval.py`, and gates top-level phase-sum
sanity. E03's `--wait-semantic` and `--unique-queries` harness modes, its
semantic-ready no-deferred-lane assertion, repeated resume sampling, and an
explicit estimated embedding-spend field are also implemented.

Parts (b)-(d) are now wired into `performance_eval.py`. The 64K/640K inner
latency tier is blocking; request-scoped SQLx completion events are counted
without enabling SQL logs and emitted as `query_count`; the six-operation
contract is checked in at `eval/query_budgets.json`; and the 64K/640K plan gate
fingerprints the authoritative migration function bodies and installed
`pg_proc.prosrc` before checking the GIN/HNSW plans. PostgreSQL deliberately
shows a `SECURITY DEFINER` SQL function as a `Function Scan`, so the plan gate
has two explicit halves: EXPLAIN the callable as `app_ro`, then EXPLAIN the
fingerprinted function body under its owner and `row_security=off` semantics.
Both use the production request GUCs, including
`hnsw.iterative_scan=relaxed_order`.

The code-shape budgets are intentionally fail-closed on the first isolated
run. They are not yet called measured baselines: the coordinated 3,340/64K
run must record the observed counts, confirm the pinned exact/upper values,
and update only a demonstrably wrong budget with an evidence artifact. No
live Nyx service or database was touched while implementing the gate.

The first current-build isolated 64K run calibrated the two provisional
misses: write is exactly 14 statements (30/30 observations), and checkpoint is
28 statements with no explicit `source_refs`. An independent 1K/10K/64K E02
calibration recorded the same shape. The compact evidence record is
`results/2026-07-27-e03-query-budget-calibration.json`; acceptance still
requires a clean rerun against the measured contract.

### (b) Regression-tier gates at 64K and 640K

New tier pinned at roughly 5-10x the v8 measured p95s, cited above:

| Operation | v8 measured p95 | Regression gate | Existing hard SLO |
|---|---|---|---|
| open | 59.7ms | ≤500ms | ≤5,000ms |
| search / broad | 53.1 / 54.8ms | ≤500ms | ≤3,000ms |
| exact read | 16.2ms | ≤100ms | ≤1,000ms |
| checkpoint | 17.1ms | ≤200ms | ≤2,000ms |
| resume | 35.2ms | ≤400ms (≤150ms once D03 lands) | — |
| concurrent/unrelated write | 29.0ms | ≤500ms | — |
| concurrent search | 100.9ms | ≤750ms | — |

Gates run at 64K (default `performance_eval.py` scale) per release and at 640K (`--future-soak`) per release during the Tier C shadow period (D14 authority-tier frame). The hard 5s/3s/1s/2s SLO gates are kept unchanged as the outer contract. A regression-gate breach is a build failure; raising a gate requires citing a new baseline artifact in the PR. Both v5/v7 write regressions would have failed the ≤500ms unrelated-write gate at either scale.

### (c) Per-operation round-trip/query-count budget assertions

The 07-26 failure class — unbudgeted synchronous work — is scale-dependent in latency but scale-independent in shape: it always adds statements to the request path. So we gate the shape, deterministically:

- Count SQLx statement-completion events within the request task scope (no schema; counter exposed alongside `timings_ms` as `query_count`). This includes authentication, transaction context/setup, application SQL, and `COMMIT`; SQLx 0.9 does not emit an event for its protocol-level `BEGIN`, and detached usage bookkeeping is intentionally excluded.
- Build item: measure current statement counts for open, search, read, write, checkpoint, resume on the clean 3,340-record fixture (results/2026-07-27-3340-clean-30-sample.json corpus) and record them as the baseline in a checked-in budget file (eval/query_budgets.json).
- Gate: each operation's count must equal the budget exactly (or ≤ budget where counts are legitimately variable, e.g. batched queries ≤16). Any change requires editing the budget file in the same PR — a visible, reviewable diff instead of a silent accretion.

This catches the 07-26 class at any corpus size, including a 3-sample `--quick` run on a laptop, without waiting for a 640K soak. It also satisfies the standing hard constraint that per-operation round-trip/query-count budget assertions accompany latency gates.

### (d) EXPLAIN plan-assertion gate

At 64K, reproduce the SECURITY DEFINER candidate-function SQL as the app role, with production GUCs set (`SET hnsw.iterative_scan = relaxed_order;` plus any session GUCs the API sets), and run `EXPLAIN (FORMAT JSON)`. Assert:

- lexical lane: Bitmap Index Scan on the GIN FTS index over `search_chunks`;
- semantic lane: Index Scan using the HNSW index;
- no Seq Scan node on `search_chunks` anywhere in either plan.

This guards against silent planner flips (statistics drift, index bloat, a migration dropping an index) that latency percentiles only reveal after the fact at scale.

Lockstep-maintenance cost, acknowledged: the function bodies live in
migrations 0051/0055, while the request path invokes them from
`simple_core.rs`. The default and D10 generation-piggyback call text is now
centralized in `apps/api/src/retrieval_sql.rs`; the plan contract fingerprints each
authoritative migration body and compares that fingerprint to the installed
`pg_proc.prosrc` before any plan can pass. A migration, installed-database, or
invocation drift is therefore a blocking, visible failure.

## What this does NOT change

- No schema expansion: `timings_ms`/`query_count` are response metadata from in-process counters.
- Zero context bytes change: candidates, excerpts, budgets, ranking, caps (128/96,000/2,400/≤3) are untouched. A byte-diff test asserts response context is identical with the flag on and off.
- Markdown authority, checkpoint format, and the MCP contract are untouched; semantic stays off the Tier B critical path.
- The existing hard SLO gates remain; this adds an inner tier, it does not replace the outer contract.

## Failure-mode analysis

- 07-26 unbudgeted bookkeeping: directly countered by (c); (b) at 640K is the backstop, per the v5/v7 evidence that only the soak caught the write regressions.
- Dedup revert / paraphrase loss / overfetch: not implicated — D09 shapes no context. The byte-diff assertion makes this checkable, not assumed.
- v6 recent-first collapse (Star Rupture 0/3): D09 explicitly does NOT guard quality. A plan flip is caught by (d); a ranking change that is fast but wrong passes every gate here. Quality regressions remain the province of n≥3 paired draws (E01-paired-draw-machinery-and-baseline.md).
- Risk introduced by D09 itself: instrumentation overhead on the hot path (mitigation: monotonic clock reads and integer counters only; overhead must be invisible at the ≤100ms exact-read gate) and `timings_ms` leaking into agent context (mitigation: the byte-diff test covers hydrated content; checkpoint text truncation path asserted free of the field).

## Acceptance gates

1. `timings_ms` phases sum to within 5% of `total` across a 30-sample 64K run; per-phase p50/p95/p99 reportable by `performance_eval.py`.
2. Byte-diff: context-bearing response fields identical with `observability.timings_ms` on/off.
3. Regression tier (table above) green at 64K, 30 samples, clean git tree fingerprint; 640K soak green before any Tier C shadow release.
4. `eval/query_budgets.json` exists for all six operations; budget assertions are wired into `performance_eval.py`; the checked negative test proves a single over-budget query fails. Final acceptance still requires the isolated run to turn the pinned code-shape values into measured baselines.
5. EXPLAIN gate green at 64K including the SQL-drift fingerprint check; checked negative plan fixtures prove a missing expected index or `Seq Scan` on `search_chunks` fails without mutating any database.

## Rollout and kill switch

Land (a) first — E03 depends on it. Flag `observability.timings_ms` defaults on; kill switch strips the field with no other behavior change. Gates (b)-(d) land in `performance_eval.py` and run per release: 64K always, 640K per release during shadow. No deploy is needed to disable emission; gates are CI-side and carry no runtime risk.

## References

- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json (pinned baselines)
- results/2026-07-27-3340-clean-30-sample.json (clean-fixture baselines; query-count baseline corpus)
- v5/v7 future-soak JSONs (write-path regressions: 3,404ms / 3,170ms unrelated-write p95)
- 2026-07-26 production collapse (unbudgeted synchronous bookkeeping) — vault incident note
- apps/api/src/simple_core.rs; migrations 0051-0055 (candidate SQL, GUCs)
- E03-semantic-ready-latency-profile.md (consumer of (a)); E01-paired-draw-machinery-and-baseline.md (quality counterpart); E10-combined-preflight.md (final combined gate); D03 (resume optimization, gate tightens to ≤150ms when it lands); D14 (authority tiers)

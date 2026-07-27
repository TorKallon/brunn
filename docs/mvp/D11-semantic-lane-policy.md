# D11 — Semantic Lane Policy: Existence First, Then Bounded Acceleration

Status: Implemented behind default-off flags — E09 not run
Date: 2026-07-27
Depends on: D14 (D14-migration-and-authority-tiers.md)
Gated by: E09 (E09-semantic-existence-experiment.md)
Runtime flag: semantic_lane, embed_cache, semantic_deadline_ms; backfill throttle under embedding_backfill_guard (canonical name shared with D12-operational-simplification.md)

Implementation note (2026-07-27): the simple workspace now applies
`semantic_lane` to both `open` and `search`, including semantic-only requests;
the default is off. `embed_cache` fronts query embeddings with the specified
model/dimension/query key, 4,096-entry LRU, seven-day positive TTL, and
60-second negative TTL. A positive `semantic_deadline_ms` bounds embed plus
vector lookup; `0` selects the E09 unbounded arm while retaining the outer
2.5-second lane timeout. Timed-out cached calls continue only long enough to
populate the query cache. `/ready` and authenticated `/v1/status` expose the
flag state, build revision, and process counters used by the fail-closed E09
harness.

The canonical backfill guard now stops embedding-job claims when off and, when
on, caps publications at 64 chunks with at least 250ms between full batches.
The configured foreground open/search p95 limits are exported as runtime
policy. Cross-process rolling-p95 sampling and automatic pause remain a D12
worker/telemetry integration item; neither this implementation nor E09 may
claim acceptance gate 5 until that sampler exists.

## Problem and evidence

The semantic lane is the only retrieval component with zero supporting eval evidence. Every strong quality result — the 57-case strict draw (legacy 170/228, simplified 160/228, direct Markdown 171/228), the matched repeat (46/64 vs 45/64), the interface run (native API 186/228 vs files 194/228) — was measured exact+lexical with embeddings pending. All latency baselines are the same: the v8 640K soak (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json: open p95 59.7ms, search 53.1ms) and the clean fixture (results/2026-07-27-3340-clean-30-sample.json) were run with the semantic lane emitting its semantic_unavailable notice. NO semantic-ready profile exists. The research record additionally says embedding similarity is miscalibrated for supersession — the exact discrimination our chronic failures need.

Meanwhile the lane carries a concrete cost: once any semantic coverage exists, every search makes a synchronous, uncached OpenAI embedding call (apps/api/src/simple_core.rs:3005) — an unbounded network round-trip on a hot path whose measured p95 is 53.1ms and whose hard gate is 3,000ms. One slow or failing OpenAI call converts a 50ms operation into a multi-second one. This is structurally the same class of defect as the 2026-07-26 production collapse (unbudgeted synchronous bookkeeping) and the v5/v7 write-path regressions (3,404ms and 3,170ms unrelated-write p95, per the v5 and v7 future-soak result JSONs in the same series as the v8 file).

So the design question is ordered: (1) does the lane improve reasoning quality at all? (2) only if yes, how do we run it without ever gating a response on it?

## Design

### (a) Existence question first

E09 (E09-semantic-existence-experiment.md) decides ship or cut before any tuning. The null hypothesis is that exact+lexical alone — the configuration behind every measured result — is sufficient. Under the project rule that every context change is guilty until proven (2026-07-22 dedup revert), the semantic lane is treated as an unproven context change even though it already exists in code. The cheapest available simplification is deleting it from the hot path; E09 explicitly permits that conclusion. Nothing below is built into the default-on path until E09 reports.

### (b) In-process LRU query-embedding cache (flag: embed_cache)

If the lane survives E09, the synchronous uncached call at simple_core.rs:3005 is fronted by an in-process LRU cache:

- Key: (embedding_model_id, dims, sha256(query after trimming and whitespace-collapsing)).
- Value: 1536-dim f32 vector, ~6KB.
- Capacity: 4,096 entries (~25MB resident), LRU eviction.
- TTL: 7 days.
- Model rotation: the model id and dims are part of the key, so rotating the embedding model self-invalidates — no flush logic.
- Failures: a 60-second negative cache per key on embed errors, so a flapping OpenAI endpoint cannot generate a retry storm; the lane defers (see c) and the gap notice stays visible.
- Storage: process memory only. NO DB table — this respects the no-schema-expansion constraint (in-process caches are explicitly allowed).

### (c) Bounded semantic deadline (flag: semantic_deadline_ms)

Lanes already run concurrently under RETRIEVAL_LANE_TIMEOUT (~2.5s). Inside that, the semantic lane gets its own budget, semantic_deadline_ms (initial value 300ms), covering embed + HNSW probe end to end. On a cache hit the embed cost is ~0 and 300ms is ample for HNSW at owner scale. On expiry:

- The response returns immediately with exact+lexical results plus an explicit
  `retrieval_lane_deferred` semantic gap. This extends the existing lane-gap
  shape so evaluation can distinguish a deadline deferral from missing
  coverage while preserving successful-lane results.
- The embedding call completes asynchronously and lands in the cache, so a repeated or refined query is warm.

This makes "accelerator, never a gate" mechanical rather than aspirational: no code path exists in which a response waits past the deadline for OpenAI. The 300ms number is a starting point only — E09 tunes it (300→600→1,000ms stepping) rather than us guessing. The response contract extends the existing lane-gap vocabulary without changing successful candidate shapes. Per-lane metrics gain a semantic_deferred counter and cache hit/miss counters (in-process, exported with existing lane metrics).

### (d) Backfill rate limit and foreground-latency guard

Initial owner-corpus embedding is exactly the unbudgeted-background-work failure class (07-26 collapse; v5/v7 contention). Backfill therefore runs: batches of ≤64 chunks, ≥250ms inter-batch sleep, and a guard that pauses backfill whenever the rolling 60s foreground open or search p95 exceeds 2x the v8 baseline (i.e. ~120ms open / ~106ms search). All three numbers are runtime config under the `embedding_backfill_guard` flag — the canonical name, shared with D12-operational-simplification.md; one guard, one name in config. Backfill progress is derivable from search_chunks state — no new durable bookkeeping.

### (e) Tier ordering

Per D14 (D14-migration-and-authority-tiers.md), semantic stays OFF the Tier B critical path. Tier B entry requires the crude char budget and write canaries, not embeddings; semantic_unavailable is an acceptable steady state for the daily driver. Semantic ships, if at all, as a Tier C-era enhancement after E09.

## What this does NOT change

- No schema expansion: the cache is process memory; backfill uses existing search_chunks rows.
- Scoring formulas, caps, and budgets are untouched (semantic 2.0+(1−distance)+bonus−penalty; 128 candidates / 96,000 excerpt chars / 2,400 per excerpt; open ≤32/≤8/24,000).
- Markdown-authority round-trip: nothing durable is added; a rebuild-from-vault reproduces all state.
- Existing MCP operations and workspace routes are unchanged. `memory.open`
  gains the same optional retrieval-mode restriction already supported by
  search, and clients already tolerate lane-gap notices.
- No validity intervals, no graph database, no synchronous global consistency; dreaming stays paused.

## Failure-mode analysis

- Dedup revert (2026-07-22): cutting the lane reduces context, and context reductions have hurt before. That is why cut-vs-ship is decided by E09's n≥3 paired draws with McNemar, never by this doc.
- v6 recent-first collapse: v6 changed ranking and hid authoritative sources. The deadline never reorders results — it only omits one lane and says so via the gap notice, a state all existing evidence was gathered under. No usage/recency signal is introduced.
- 07-26 bookkeeping collapse / v5-v7 regressions: addressed head-on by (c) and (d); the concurrent write/search probe and the 640K soak remain the enforcement mechanism, since only the soak caught the prior regressions.
- Overfetch (RuptureOps ~70,814 vs legacy 41,441 service chars/case): the semantic lane adds candidates; cutting or deferring it can only shrink overfetch. E09 records chars/case per arm.
- Paraphrase/context-compilation loss: 21/22 disputed answers already had an accepted source in context — losses are compilation, not retrieval — so a semantic recall gain has no demonstrated path to score gain. This is the core of the existence doubt.
- Cache staleness: only query embeddings are cached; corpus vectors are unaffected. Model rotation is handled by the key. Worst case is a 7-day-old query vector from an unchanged model — benign.
- Persistent OpenAI outage: 60s negative cache bounds retry pressure; the gap notice keeps degradation observable rather than silent.

## Acceptance gates

Deterministic (pre-experiment):

1. Unit tests: key normalization (whitespace/trim), LRU eviction at 4,096, TTL expiry, negative-cache window, model-id self-invalidation.
2. Deterministic mock-embedder deadline test with injected latency greater than `semantic_deadline_ms`: the bounded future defers, and the cache is populated asynchronously afterward. The full HTTP/mock-server acceptance probe remains part of E09 stack qualification.
3. Round-trip budget assertion: a search performs ≤1 embed call, exactly 0 on cache hit (per the constraint that query-count budgets accompany latency gates).
4. performance_eval.py semantic-failure probe passes using the mock server as --semantic-failure-start/stop-command hooks; 64K 30-sample p95s within hard gates with the lane on.
5. Backfill guard test: synthetic foreground load above threshold pauses backfill within one batch. **Pending D12 cross-process foreground sampler; the existing code proves kill-switch, batch-cap, and pacing behavior only.**

Experimental: E09 acceptance criteria. Ship the lane on the hot path only if E09 says so; otherwise remove it from the default search path (flag stays for research).

## Rollout and kill switch

Order: build (b)-(d) behind flags → run E09 → decision. Defaults until E09 passes: semantic_lane=off in production (exact+lexical, the exact measured-baseline configuration), embed_cache=on whenever the lane is on, semantic_deadline_ms=300. semantic_lane=off is the kill switch: runtime config, no deploy, and it restores precisely the configuration of every recorded baseline. OWNER DECISION: whether a failed E09 deletes the lane code or leaves it flag-gated for future research.

## References

- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json
- results/2026-07-27-3340-clean-30-sample.json
- v5 and v7 future-soak performance JSONs (same results/ series as v8)
- apps/api/src/simple_core.rs:3005 (synchronous uncached query embed)
- tests/mock_openai_embeddings.py
- D14-migration-and-authority-tiers.md; E09-semantic-existence-experiment.md; D12-operational-simplification.md (shared embedding_backfill_guard)
- Vault: 2026-07-22 dedup revert note; 2026-07-26 production collapse note; v6 recent-first negative result

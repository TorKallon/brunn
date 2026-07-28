# D01 — Budget-Contracted Retrieval

Status: Proposed — not started
Date: 2026-07-27
Depends on: D02 (ship-pairing; D02-verbatim-span-contract.md — see the exact-value pointer caveat below)
Gated by: E04 (E04-result-budget-experiment.md)
Runtime flag: `search.fair_share`, `search.top1_hydration`, `search.char_cap` (three independent kill switches)

## Problem and evidence

Two documented defects share one root cause: the search response spends its budget in the wrong places.

1. **Starvation.** The 128-candidate / 96,000-char response budget is consumed first-query-first across up to 16 batched queries — a known defect. Later queries in a batch can receive nothing even when they have strong matches.
2. **Overfetch.** RuptureOps pulls ~70,814 service chars/case vs legacy 41,441 — the leading quality risk. Yet 21/22 disputed simplified answers already had a rubric-accepted source in returned context: losses are context compilation, not retrieval. On the identical simplified core, the files interface scored 194/228 vs native API 186/228. The file-shaped reading pattern — few complete documents instead of many fragments — wins.

This design merges the top "stronger than Markdown" item (deliver complete artifacts) with the overfetch fix. It is ONE change gated by ONE experiment, E04 (E04-result-budget-experiment.md).

## Design

Rank like a service, read like a file. Four bounded mechanisms, all applied at response assembly over already-fetched lane candidates in `apps/api/src/simple_core.rs`. No new queries are introduced except one batched hydration fetch.

**(a) Fair-share allocation** (flag `search.fair_share`). For N batched queries, each query's floor share is `floor(128/N)` candidates and `floor(96,000/N)` chars. Fill round-robin by rank: query 1 rank 1, query 2 rank 1, … query N rank 1, then rank 2, and so on (queries in request order, candidates in lane-score order). A query that exhausts its matches surrenders its unused share; surplus is redistributed in the same round-robin order only after every query has had the chance to consume its floor. Deterministic, O(candidates), in-memory.

**(b) Top-1 complete-artifact hydration** (flag `search.top1_hydration`). Per query, the top-ranked candidate is promoted to representation `complete_source` when `size_bytes <= min(24,000, remaining global char budget)`, reusing open's hydration path and its `MAX_OPEN_COMPLETE_SOURCE_CHARS` bound. All promotions across the batch are fetched in ONE batched round trip (entry_id/version list), never per-candidate. When a query's top-1 is promoted, its remaining candidates are demoted to pointer-only leads (`path | version N | sha256:… | score`, no excerpt), reusing the existing `evidence_leads` shape. If the top-1 does not fit, no promotion occurs and the query returns excerpts as today.

**(c) Token-budget-tied cap** (flag `search.char_cap`). When the session declares `token_budget`, total response chars cap at `token_budget*4` — the same conversion the checkpoint path already uses. Default assumption 12,000 tokens → 48,000 chars, deliberately near the legacy ~41.4K chars/case level that beat the simplified core on RuptureOps, and the concrete implementation of D14's Tier B entry criterion "crude open/search char budget near legacy ~41.4K" (D14-migration-and-authority-tiers.md). OWNER DECISION: the assumed default `token_budget` when the session declares none (proposed 12,000).

**(d) Section demotion.** Beyond the top-8 entries per query, `additional_sections` become heading-only leads (heading + path + version, no body). Governed by the `char_cap` flag plus a numeric knob `section_demotion_top_n` (default 8; unset disables), so E04's bisection can isolate it.

**Contract deltas.** Request: optional `token_budget` (already exists on checkpoint). Response: candidates carry `representation ∈ {complete_source, excerpt, pointer_lead, heading_lead}`. Existing hard caps (≤128 candidates, ≤96,000 chars, ≤2,400 chars/excerpt, ≤3 sections/entry) remain maxima; the mechanisms only allocate within them.

**The gate binds on the BATCHED worst case.** Naively, 16 queries × 24,000-char promotions = 384,000 chars. Promotion is therefore checked against the remaining GLOBAL budget — `min(96,000, token_budget*4)` minus chars already committed — so at the 48,000-char default at most one full 24K promotion fits alongside excerpts; further promotions degrade to excerpt, then pointer, in round-robin order. The deterministic acceptance gate tests exactly this case.

**Exact-value pointer caveat.** Pointers preserve no verbatim values. A demoted rank-2 source holding the exact date/ID/number a rubric wants becomes invisible without a follow-up read. This design must ship paired with D02 (D02-verbatim-span-contract.md), and E04 tracks exact-value claim slots separately.

## What this does NOT change

- No schema expansion; no new tables, columns, or validity intervals. Allocation is in-process arithmetic over fetched candidates.
- Lane scoring untouched: exact flat 10.0, lexical 3.0+ts_rank_cd, semantic 2.0+(1−distance). No recency or usage ranking — the v6 recent-first experiment is a completed negative.
- open/read/checkpoint/changes contracts unchanged; the exact lane still returns the precise entry.
- Markdown authority and rebuild-from-vault round-trip unaffected (no new durable metadata). Semantic stays off the Tier B critical path.

## Failure-mode analysis

- **Dedup revert (2026-07-22) — the shape this most resembles, addressed head-on.** Cross-query dedup removed context and put nothing in its place; quality dropped and it was reverted; every context reduction is guilty until proven. D01 differs in three specific ways. First, it is a trade, not a cut: chars removed from ranks 2..k fund a complete top-1 artifact — deepening context in the direction the files condition proved out (194 vs 186 on the same core). Second, everything removed leaves a durable, followable pointer or heading lead; dedup left nothing. Third, it ships behind three independent kill switches and is decided solely by E04's n≥3 paired draws with McNemar plus a 5-draw chronic repeat — never by single-draw deltas (noise floor ±3-5 claims). If E04 shows the dedup pattern, the flags come off and the doc records a second confirmed negative.
- **Paraphrase/exact-value loss.** Demotion drops verbatim values. Mitigations: the exact lane is untouched; remaining excerpts are verbatim source text (no summarization is introduced anywhere); D02 pairing; E04's separate exact-value slot metric.
- **v6 recent-first collapse.** D01 reorders nothing; it reallocates within existing rank order. Residual risk: top-1 hydration amplifies a ranking mistake (24K chars spent on the wrong doc, right doc reduced to a pointer). E04's arm B vs C isolates hydration, and the chronic cases (the Star Rupture 0/3 shape) are the tripwire.
- **07-26 bookkeeping collapse.** No unbudgeted synchronous work: exactly one additional batched round trip, enforced by a per-operation query-count assertion accompanying the latency gates.
- **Overfetch.** Mechanisms (c)+(d) bound it directly; E04 measures chars/case per arm.

## Acceptance gates

Deterministic (CI, no model runs):

1. Batched worst case: 16 queries each with a ≥24K top candidate → total response chars ≤ `min(96,000, token_budget*4)`, degradation order deterministic.
2. Starvation fix: 16-query batch where query 16 has matches → query 16 receives at least its floor share.
3. Round-trip budget: search executes existing lane queries plus at most one
   hydration batch. The request-scoped SQL counter observes that batch as the
   exact set `{0,+5}`: zero when no candidate is hydrated, or context
   validation + context setup + timeout setup + one batched hydration
   `SELECT` + `COMMIT`. E04 requires at least one `+5`; no per-candidate query
   is allowed.
4. `python performance_eval.py run --label d01-flags-on-soak --future-soak --out results/2026-MM-DD-d01-flags-on-soak.json` (30 samples, definitive) with all flags on: every existing gate passes; no drift vs v8 baseline — search p95 53.1ms, open 59.7ms, concurrent write 29.0ms / search 100.9ms (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json); flat-file control unchanged.

Experimental: E04 (E04-result-budget-experiment.md) acceptance criteria, in full.

## Rollout and kill switch

All three flags default off; with flags off, behavior is identical to the measured baselines. Enable on Nyx only for E04 arms. Production enablement follows the winning arm (B configuration before C). Each flag is a runtime-config kill switch — disable without deploy, independently: `fair_share` restores first-query-first, `top1_hydration` restores excerpt-only candidates, `char_cap` restores the flat 96,000-char cap and re-enables all sections. `char_cap` additionally satisfies D14 Tier B entry; the other two ship only on E04 evidence.

## References

- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json (soak baselines)
- results/2026-07-27-3340-clean-30-sample.json (clean-fixture open p95 26.278ms; direct Markdown discovery 72.985ms)
- 57-case strict draw evidence: native 186/228 vs files 194/228; 21/22 accepted-source-in-context; RuptureOps ~70,814 vs legacy 41,441 chars/case
- Vault notes: 2026-07-22 dedup revert; v6 recent-first collapse (Star Rupture 0/3); 2026-07-26 bookkeeping collapse
- E04-result-budget-experiment.md; D02-verbatim-span-contract.md; D14-migration-and-authority-tiers.md

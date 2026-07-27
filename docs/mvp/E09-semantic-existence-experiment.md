# E09 — Semantic Lane Existence Experiment: Ship, Bound, or Cut

Status: Specified — not run
Date: 2026-07-27
Gates: D11 (D11-semantic-lane-policy.md)
Phase: 1 (requires the D11 flag build and E03 semantic-ready corpus; not run)

## Question

Does the semantic lane improve reasoning quality at all — and if it does, does the bounded deadline+cache variant (D11 c/b) preserve that improvement? Every strong quality and latency result to date is exact+lexical with embeddings pending; the lane's cost (a synchronous OpenAI call at apps/api/src/simple_core.rs:3005) is certain while its benefit is unmeasured. The explicitly allowed — and cheapest — conclusion is: DO NOT SHIP the semantic lane on the hot path.

## Preconditions and build items

1. E03 complete (E03-semantic-ready-latency-profile.md, including its build item 6 eval-corpus backfill): the eval corpus fully embedded, verified by zero semantic_unavailable notices on a warm probe query in semantic arms. (L, already scoped under E03 — not counted here.)
2. D11 cache + deadline behind flags: embed_cache, semantic_deadline_ms, wired at the query-embed call (simple_core.rs:3005) and lane dispatch under RETRIEVAL_LANE_TIMEOUT. (M — apps/api/src/simple_core.rs.)
3. No-semantic arm: `semantic_lane=off` is the mechanical server-side kill
   switch for both `open` and `search`. The harness additionally forces
   `modes=["exact","lexical"]` into every adapter open/search request so the
   request transcript proves the intended arm. (Implemented.)
4. n≥3 paired-draw aggregator — per-case win/loss/tie, exact-binomial McNemar, case-level bootstrap CIs, stdlib only. Does not exist yet. (S — eval/aggregate_draws.py, the shared build item specified in E01-paired-draw-machinery-and-baseline.md; build once, one name.)
5. Per-run export of cache hit rate, semantic-deferral rate, and per-lane latency into the run JSON. (S — metrics already exist per-lane; export plumbing only.)
6. Clean git tree for every run (performance_eval implementation-fingerprint rule applies to quality runs here too).

## Arms

- A. no_semantic — exact+lexical only via modes; the configuration of all measured baselines.
- B. unbounded_semantic — semantic_lane=on, embed_cache=off, no semantic deadline (only RETRIEVAL_LANE_TIMEOUT bounds the lane). Current code behavior with coverage present.
- C. deadline_cache — semantic_lane=on, embed_cache=on, semantic_deadline_ms=300.

## Corpus and fixtures

- Quality suites: agent-work (13 cases / 52 claims), rupture (12 / 48), recent (12 / 48) = 37 cases / 148 claims per draw. Personal-coordination is excluded for cost control and because it carries no semantic-lane-specific hypothesis (its chronic case, coord-deadline-readiness, is a prospective-memory failure — E08's domain, not a retrieval-lane question). Transitions are excluded because they are stuck at 0/5 on claim-slot omissions unrelated to retrieval lanes.
- Condition: service_api only (the lane under test lives behind the service).
- Corpus: the standard eval corpus for these manifests, embedded per E03. Coverage check before any semantic-arm draw (precondition 1).
- Latency fixture: performance_eval 64K scale, 30 samples (definitive), per arm.

## Procedure

1. Verify clean git tree, immutable API image, and exact image-revision match.
   Run the coverage probe; abort if any `semantic_unavailable` at warm start in
   arms B/C. `--e09-arm` fails before reasoning on source dirtiness, API build
   mismatch, or runtime flag drift.
2. Set arm environment on the disposable stack and restart that stack (no new
   image/deploy): A = lane off/cache on/deadline 300; B = lane on/cache
   off/deadline 0; C = lane on/cache on/deadline 300. For each arm ∈
   {no_semantic, unbounded_semantic, deadline_cache}, each suite manifest ∈
   {work, rupture_ops, recent_work}, each draw N ∈ {1,2,3}:

   `python3 agent_work_eval.py --manifest eval/work_cases.json run --condition service_api --service-protocol simple --e09-arm <arm> --concurrency 3 --timeout 360 --run-id e09-<arm>-work-draw<N> --out results/2026-MM-DD-e09-<arm>-work-draw<N>.json --report results/2026-MM-DD-e09-<arm>-work-draw<N>.md`

   (substitute eval/rupture_ops_cases.json and eval/recent_work_cases.json with matching slugs: results/2026-MM-DD-e09-<arm>-<suite>-draw<N>.json). Model comes from the manifest (gpt-5.6-sol). 27 harness invocations total.
3. Interleave draws across arms (arm order rotated per draw) so time-of-day drift is not confounded with arm.
4. After each run, confirm the run JSON contains cache hit rate (arm C), deferral rate (arms B/C), and per-lane latency.
5. Latency:
   `python3 performance_eval.py run --protocol simple --e09-arm <arm> --label e09-<arm>-64k-latency --scales 64000 --samples 30 --api-container <api-container> --db-container <db-container> --out results/2026-MM-DD-e09-<arm>-64k-latency.json`
   per arm under that arm's flags. Definitive mode also requires the
   semantic-failure start/stop hooks already specified by the performance
   harness.
6. Aggregate: `python eval/aggregate_draws.py results/2026-MM-DD-e09-*-draw*.json --out results/2026-MM-DD-e09-aggregate.json` — paired per-case win/loss/tie and exact-binomial McNemar for all three arm pairs (A-B, A-C, B-C), pooled across the 3 draws; bootstrap CIs per suite.
7. Deadline stepping: only if C loses to B with McNemar significance, step semantic_deadline_ms 300→600→1,000, re-running ONLY the losing suite (3 draws per step), and re-test the B-C pair on that suite.

## Metrics

- Primary: per-case claim scores per suite; pooled paired win/loss/tie and McNemar p for A-B, A-C, B-C. Single-draw deltas are noise (documented ±3-5 claim swing); only the n≥3 pairing is load-bearing.
- Cache hit rate (arm C), per draw and pooled. Decision input: if near zero, agent queries are high-entropy and the cache is dead weight — cut the cache, keep only the deadline.
- Semantic-deferral rate (arms B/C), warm-state target <10% for C.
- Latency: 64K 30-sample p95 open/search/read/checkpoint per arm, against hard gates (search ≤3,000ms) and the v8 640K reference (search 53.1ms).
- Overfetch: service chars/case per arm (RuptureOps baseline ~70,814 vs legacy 41,441).

## Acceptance criteria

- Ship semantic (some form) only if B or C beats A with exact-binomial McNemar p < 0.05 pooled over n=3 paired draws, with no individual suite showing a significant regression against A.
- If no semantic arm beats A: cut the lane from the default hot path (D11 rollout section). This outcome is a success, not a failure — it is the cheapest simplification available.
- C is the shipping variant only if C is not significantly worse than B (directly, or after deadline stepping per Procedure 7). If C still loses at 1,000ms, OWNER DECISION: ship B unbounded (accepting the synchronous-call risk D11 documents) or cut.
- Cache retained only if pooled hit rate ≥5%; below that, ship deadline-only.
- Arm C must show deferral <10% warm and all 64K p95s within hard gates; otherwise C is not shippable regardless of quality score.

## Cost preflight and ceiling

- Reasoning runs: 37 cases × 3 draws × 3 arms = 333 case-runs. At the audited ≈$0.24/agent-run equivalent (470-run audit, $113.18): 333 × $0.24 ≈ $79.92 ≈ $80.
- Deadline stepping contingency: one suite (12-13 cases) × 3 draws × $0.24 ≈ $9 per step; two steps ≈ $18. $80 + $18 = $98.
- Hard ceiling: $100 all-in for reasoning. Stop at the ceiling even mid-arm.
- Subscription rule: ALL reasoning runs execute via the ChatGPT-authenticated Codex subscription, fail-closed (require_codex_subscription rejects API keys). No run may be re-pointed at usage-billed API to "finish the draw".
- Embeddings (exempt, usage-billed OpenAI, listed separately): E03 establishes
  the semantic-ready profile, but the quality harness still creates isolated
  per-case users. At the checked-in corpus character counts and the conservative
  four-characters/token estimate, the two semantic arms across three draws
  embed about 33.6M tokens, or about **$0.67** at $0.02/M tokens, before retries.
  Query embeddings are much smaller; retain **$2 as the conservative E09
  embedding ceiling** and report provider receipts when available. This is well
  below the owner's $20 notification threshold; stop and notify before
  proceeding if a preflight or observed retry pattern raises the estimate above
  $20.

## Abort criteria

- Reasoning spend reaches $100.
- Any run attempts API-key billing (require_codex_subscription trip) — halt, fix auth; never proceed on usage billing.
- semantic_unavailable at warm start in arms B/C (E03 precondition broken) — halt before spending draws.
- Deferral rate >50% warm in arm C (broken deadline build; quality data would be meaningless).
- Any arm's 64K search p95 exceeds the 3,000ms hard gate — stop that arm's quality draws, file a defect against D11.
- Dirty git tree or flag drift detected mid-experiment (fingerprint mismatch between draws) — invalidate and restart the affected arm.

## Reporting

The run record must contain: git commit hash and flag settings per arm; all 27+ artifact paths (quality draws, latency runs, any stepping runs) under the results/2026-MM-DD-e09-* naming; per-arm per-draw per-suite claim totals; the three paired McNemar tables with win/loss/tie counts and bootstrap CIs; cache hit and deferral rates; the 64K latency table per arm against hard gates and the v8 reference; overfetch chars/case per arm; actual spend vs the $80 preflight and the separate embedding spend; and a single ship/bound/cut recommendation mapped to D11's acceptance gates, including the cache-retention and deadline-value decisions.

Each quality and performance JSON now also records authenticated runtime flag
provenance, API build revision, before/after semantic counters, counter deltas,
cache-hit rate, and deferral rate. These are process counters; isolate and
serialize E09 service stacks so unrelated traffic cannot contaminate a run.

## References

- D11-semantic-lane-policy.md; D14-migration-and-authority-tiers.md; E03-semantic-ready-latency-profile.md
- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json
- Decisions.md cost rules (subscription fail-closed; embeddings exempt)
- Vault: 2026-07-22 dedup revert; noise-floor record (agent-work native 40→47→44→43→47)

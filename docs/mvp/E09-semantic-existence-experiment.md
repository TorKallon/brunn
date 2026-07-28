# E09 — Semantic Lane Existence Experiment: Ship, Bound, or Cut

Status: Prerequisite abort — E03 Mode 2 failed and quality backfill was not run
Date: 2026-07-27
Gates: D11 (D11-semantic-lane-policy.md)
Phase: 1 (requires the D11 flag build and E03 semantic-ready corpus; not run)

**CURRENT PREREQUISITE ABORT (2026-07-28):** Do not run E09. E03's fully
indexed Mode 2 failed its blocking zero-deferred-lane gate, and the quality
backfill was therefore not run. There is no accepted semantic-ready
prerequisite artifact for these draws. This abort is not an E09 semantic-lane
verdict; after the E03 defect is repaired and its quality backfill passes, the
experimental intent below remains unchanged.

## Question

Does the semantic lane improve reasoning quality at all — and if it does, does the bounded deadline+cache variant (D11 c/b) preserve that improvement? Every strong quality and latency result to date is exact+lexical with embeddings pending; the lane's cost (a synchronous OpenAI call at apps/api/src/simple_core.rs:3005) is certain while its benefit is unmeasured. The explicitly allowed — and cheapest — conclusion is: DO NOT SHIP the semantic lane on the hot path.

## Preconditions and build items

1. **NOT SATISFIED.** E03 complete
   (E03-semantic-ready-latency-profile.md, including its build item 6
   eval-corpus backfill): the eval corpus fully embedded, verified by zero
   semantic_unavailable notices on a warm probe query in semantic arms. E03
   Mode 2 failed and the quality backfill was not run. (L, already scoped
   under E03 — not counted here.)
2. D11 cache + deadline behind flags: embed_cache, semantic_deadline_ms, wired at the query-embed call (simple_core.rs:3005) and lane dispatch under RETRIEVAL_LANE_TIMEOUT. (M — apps/api/src/simple_core.rs.)
3. No-semantic arm: `semantic_lane=off` is the mechanical server-side kill
   switch for both `open` and `search`. The harness additionally forces
   `modes=["exact","lexical"]` into every adapter open/search request so the
   request transcript proves the intended arm. (Implemented.)
4. Arm-aware n≥3 paired-draw aggregation, immutable arm/draw binding, and authenticated runtime snapshots are implemented; see [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
5. Per-run export of cache hit rate, semantic-deferral rate, and per-lane latency into the run JSON. (S — metrics already exist per-lane; export plumbing only.)
6. Clean git tree for every run (performance_eval implementation-fingerprint rule applies to quality runs here too).

## Arms

- A. no_semantic — exact+lexical only via modes; the configuration of all measured baselines.
- B. unbounded_semantic — semantic_lane=on, embed_cache=off, no semantic deadline (only RETRIEVAL_LANE_TIMEOUT bounds the lane). Current code behavior with coverage present.
- C. deadline_cache — semantic_lane=on, embed_cache=on, semantic_deadline_ms=300.
- C600. deadline_cache_600 — semantic_lane=on, embed_cache=on,
  semantic_deadline_ms=600. This identity does not exist as an ordinary arm:
  it is accepted only with the exact SHA-256 of a clean-source step-policy
  artifact authorizing the selected suite and case IDs.

## Corpus and fixtures

- Quality suites at the checked-in manifest revisions: agent-work (13 cases / 52 claims), rupture (12 / 48), recent (14 / 56) = 39 cases / 156 claims per draw. Personal-coordination is excluded for cost control and because it carries no semantic-lane-specific hypothesis (its chronic case, coord-deadline-readiness, is a prospective-memory failure — E08's domain, not a retrieval-lane question). Transitions are excluded because they are stuck at 0/5 on claim-slot omissions unrelated to retrieval lanes.
- Condition: service_api only (the lane under test lives behind the service).
- Corpus: the standard eval corpus for these manifests. E03 rehearses and
  prices backfill, but each E09 case is provisioned under an isolated user;
  the E09 harness must independently wait for and prove coverage before every
  semantic-arm case.
- Latency fixture: performance_eval 64K scale, 30 samples (definitive), per arm.

## Procedure

1. Verify clean git tree, immutable API image, and exact image-revision match.
   Run the coverage probe; abort if any `semantic_unavailable` at warm start in
   arms B/C. `--e09-arm` fails before reasoning on source dirtiness, API build
   mismatch, or runtime flag drift.
2. Allocate three project-scoped stacks from the shared preamble. Restart the
   relevant API process before **every artifact** so process-global semantic
   counters and cache entries cannot cross draws; do not rebuild the image.
   A = lane off/cache on/deadline 300; B = lane on/cache off/deadline 0;
   C = lane on/cache on/deadline 300. For each arm ∈
   {no_semantic, unbounded_semantic, deadline_cache}, each suite manifest ∈
   {work, rupture_ops, recent_work}, each draw N ∈ {1,2,3}:

   `python3 agent_work_eval.py --manifest eval/work_cases.json run --condition service_api --service-protocol simple --api-container "$API_CONTAINER" --e09-arm "$ARM" --experiment-arm "$EXPERIMENT_ARM" --paired-draw-id "e09-work-draw${N}" --expect-build-revision "$REV" --expect-feature-flag verbatim_spans=on --concurrency 3 --timeout 360 --run-id "e09-${ARM}-work-draw${N}" --out "results/2026-MM-DD-e09-${ARM}-work-draw${N}.json" --report "results/2026-MM-DD-e09-${ARM}-work-draw${N}.md"`

   The exact arm identities are `e09-no-semantic`, `e09-unbounded-semantic`, and `e09-deadline-cache`. `--e09-arm` derives and validates the full semantic policy before reasoning: no-semantic = lane off/cache on/deadline 300ms/backfill guard on; unbounded-semantic = lane on/cache off/no embedding deadline/backfill guard on; deadline-cache = lane on/cache on/deadline 300ms/backfill guard on. Conflicting explicit runtime expectations fail closed.

   (substitute eval/rupture_ops_cases.json and eval/recent_work_cases.json with matching slugs: results/2026-MM-DD-e09-<arm>-<suite>-draw<N>.json). Model comes from the manifest (gpt-5.6-sol). 27 harness invocations total.
3. Serialize E09 quality invocations because provider rate limits and
   process-global counters are shared. Rotate arm order A/B/C, B/C/A, C/A/B
   across draws, and rotate suite order inside each draw.
4. After each run, confirm the run JSON contains cache hit rate (arm C), deferral rate (arms B/C), and per-lane latency.
5. Run latency one stack at a time. Arm A is explicitly nonsemantic:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile default-safe --e09-arm no_semantic --label e09-no-semantic-64k-latency --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag embed_cache=on --expect-feature-flag embedding_backfill_guard=on --expect-feature-flag verbatim_spans=on --expect-runtime-config semantic_deadline_ms=300 --out results/2026-MM-DD-e09-no-semantic-64k-latency.json`.
   Arm B uses its lane-local controllable provider proxy:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical semantic --semantic-failure-probe required --semantic-failure-start-command "$SEMANTIC_FAILURE_START" --semantic-failure-stop-command "$SEMANTIC_FAILURE_STOP" --wait-semantic --query-budget-profile default-safe --e09-arm unbounded_semantic --label e09-unbounded-semantic-64k-latency --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=on --expect-feature-flag embed_cache=off --expect-feature-flag embedding_backfill_guard=on --expect-feature-flag verbatim_spans=on --expect-runtime-config semantic_deadline_ms=null --out results/2026-MM-DD-e09-unbounded-semantic-64k-latency.json`.
   Arm C is:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical semantic --semantic-failure-probe required --semantic-failure-start-command "$SEMANTIC_FAILURE_START" --semantic-failure-stop-command "$SEMANTIC_FAILURE_STOP" --wait-semantic --query-budget-profile default-safe --e09-arm deadline_cache --label e09-deadline-cache-64k-latency --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=on --expect-feature-flag embed_cache=on --expect-feature-flag embedding_backfill_guard=on --expect-feature-flag verbatim_spans=on --expect-runtime-config semantic_deadline_ms=300 --out results/2026-MM-DD-e09-deadline-cache-64k-latency.json`.
   The failure and restore commands must target only the current stack's
   proxy.
6. Aggregate an explicit quality-only array:
   `E09_QUALITY=(results/2026-MM-DD-e09-{no_semantic,unbounded_semantic,deadline_cache}-{work,rupture,recent}-draw{1,2,3}.json); python3 eval/aggregate_draws.py "${E09_QUALITY[@]}" --expected-arm e09-deadline-cache --expected-arm e09-unbounded-semantic --expected-arm e09-no-semantic --expected-arm-retrieval-modes e09-deadline-cache=exact,lexical,semantic --expected-arm-retrieval-modes e09-unbounded-semantic=exact,lexical,semantic --expected-arm-retrieval-modes e09-no-semantic=exact,lexical --out results/2026-MM-DD-e09-aggregate.json`.
7. Deadline stepping: only if C loses to B with McNemar significance, generate
   `python3 eval/e09_step_policy.py --losing-suite <suite> --out
   results/2026-MM-DD-e09-step-policy.json`, supplying actual base spend when
   available. The artifact may authorize exactly one 300→600ms step, three
   draws, and at most 12 deterministic case IDs while keeping accounted spend
   at or below $100. A truncated suite is exploratory and requires an owner
   decision after the step. Automatic 1,000ms stepping is forbidden; it needs
   new owner authorization and a new budget artifact.

   The policy output is immutable (`open(..., "x")`) and records its clean
   source revision, harness SHA-256, suite-manifest SHA-256, deterministic case
   IDs, and authorization identity. Hash that file, restart the isolated stack
   at a 600ms deadline, and execute each authorized draw with:

   `--e09-arm deadline_cache_600 --experiment-arm e09-deadline-cache-600 --e09-step-policy <policy.json> --e09-step-policy-sha256 <sha256>`

   Pass the exact `--case` arguments emitted by the policy and reuse the
   corresponding base-C `--paired-draw-id` values. The agent harness rejects
   any extra, missing, reordered, differently manifested, dirty-source, or
   differently revised selection. The 600ms latency artifact requires the same
   policy/hash pair in `performance_eval.py`.

   Aggregate only C versus C600:

   `python3 eval/aggregate_draws.py <base-C-jsons> <C600-jsons> --expected-arm e09-deadline-cache --expected-arm e09-deadline-cache-600 --expected-arm-retrieval-modes e09-deadline-cache=exact,lexical,semantic --expected-arm-retrieval-modes e09-deadline-cache-600=exact,lexical,semantic --e09-step-policy <policy.json> --e09-step-policy-sha256 <sha256> --out results/2026-MM-DD-e09-step-aggregate.json`

   The aggregator uses only the authorized subset from the immutable full-suite
   300ms draws, requires arm-complete three-draw pairing, and rejects any C600
   artifact whose run ledger is not bound to that policy. There is deliberately
   no 1,000ms arm.

## Metrics

- Primary: per-case claim scores per suite; pooled paired win/loss/tie and McNemar p for A-B, A-C, B-C. Single-draw deltas are noise (documented ±3-5 claim swing); only the n≥3 pairing is load-bearing.
- Cache hit rate (arm C), per draw and pooled. Decision input: if near zero, agent queries are high-entropy and the cache is dead weight — cut the cache, keep only the deadline.
- Semantic-deferral rate (arms B/C), warm-state target <10% for C.
- Latency: 64K 30-sample p95 open/search/read/checkpoint per arm, against hard gates (search ≤3,000ms) and the v8 640K reference (search 53.1ms).
- Overfetch: service chars/case per arm (RuptureOps baseline ~70,814 vs legacy 41,441).

## Acceptance criteria

- Ship semantic (some form) only if B or C beats A with exact-binomial McNemar p < 0.05 pooled over n=3 paired draws, with no individual suite showing a significant regression against A.
- If no semantic arm beats A: cut the lane from the default hot path (D11 rollout section). This outcome is a success, not a failure — it is the cheapest simplification available.
- C is the shipping variant only if C is not significantly worse than B directly or after the single 600ms contingency in Procedure 7. If C still loses, or the bounded subset is only exploratory, OWNER DECISION: authorize a newly budgeted investigation, ship B unbounded (accepting the synchronous-call risk D11 documents), or cut.
- Cache retained only if pooled hit rate ≥5%; below that, ship deadline-only.
- Arm C must show deferral <10% warm and all 64K p95s within hard gates; otherwise C is not shippable regardless of quality score.

## Cost preflight and ceiling

- Reasoning runs: 39 cases × 3 draws × 3 arms = 351 case-runs. At the audited ≈$0.24/agent-run equivalent (470-run audit, $113.18): 351 × $0.24 = $84.24.
- Deadline stepping contingency: at most 12 cases × 3 draws × $0.24 = $8.64 for one 600ms step. Projected maximum automatic path: $92.88. `eval/e09_step_policy.py` re-derives the base from current manifests, uses actual base spend when supplied, shrinks or blocks the step to stay within $100, and never authorizes 1,000ms.
- Hard ceiling: $100 all-in for reasoning. Stop at the ceiling even mid-arm.
- Subscription rule: ALL reasoning runs execute via the ChatGPT-authenticated Codex subscription, fail-closed (require_codex_subscription rejects API keys). No run may be re-pointed at usage-billed API to "finish the draw".
- Embeddings (exempt, usage-billed OpenAI, listed separately): E03 establishes
  the semantic-ready profile, but the quality harness still creates isolated
  per-case users. At the checked-in corpus character counts and the conservative
  four-characters/token estimate plus a 25% chunk-overlap allowance, the two
  semantic arms across three draws embed about 42.0M tokens, or about **$0.84**
  at $0.02/M tokens, before retries.
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

The run record must contain: git commit hash and flag settings per arm; all 27+ artifact paths (quality draws, latency runs, and the bounded step-policy/run artifacts if triggered) under the results/2026-MM-DD-e09-* naming; per-arm per-draw per-suite claim totals; the three paired McNemar tables with win/loss/tie counts and bootstrap CIs; cache hit and deferral rates; the 64K latency table per arm against hard gates and the v8 reference; overfetch chars/case per arm; actual spend vs the $84.24 base projection and the separate embedding spend; and a single ship/bound/cut recommendation mapped to D11's acceptance gates, including the cache-retention and deadline-value decisions.

Each quality and performance JSON now also records authenticated runtime flag
provenance, API build revision, before/after semantic counters, counter deltas,
cache-hit rate, and deferral rate. These are process counters; isolate and
serialize E09 service stacks so unrelated traffic cannot contaminate a run.

## References

- D11-semantic-lane-policy.md; D14-migration-and-authority-tiers.md; E03-semantic-ready-latency-profile.md
- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json
- Decisions.md cost rules (subscription fail-closed; embeddings exempt)
- Vault: 2026-07-22 dedup revert; noise-floor record (agent-work native 40→47→44→43→47)

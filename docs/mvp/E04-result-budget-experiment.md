# E04 — Result Budget Experiment

Status: Specified — not run
Date: 2026-07-27
Gates: D01 (D01-budget-contracted-retrieval.md)
Phase: 1 (requires flagged feature build)

## Question

Does budget-contracted retrieval — fair-share allocation + token-tied char cap + section demotion, with and without top-1 complete-artifact hydration — improve RuptureOps answer quality, or hold quality while cutting service chars/case by ≥25%, without regressing the guard suites, under n≥3 paired draws with McNemar?

## Preconditions and build items

1. (M) D01 feature behind flags `search.fair_share` / `search.top1_hydration` / `search.char_cap` in apps/api/src/simple_core.rs: response-assembly budget allocation; hydration reusing open's complete-source path (`MAX_OPEN_COMPLETE_SOURCE_CHARS`); single batched hydration fetch; `section_demotion_top_n` knob.
2. (S) n≥3 paired aggregator (known missing build item): eval/aggregate_draws.py — per-case win/loss/tie, exact-binomial McNemar, case-level bootstrap CIs, stdlib only. Shared build item, specified in E01-paired-draw-machinery-and-baseline.md; build once.
3. (S) Personal suite clean 60-claim run: the last run graded only 40/60. Diagnose with `python agent_work_eval.py regrade` on the saved answers; fix grading against eval/personal_coordination_cases.json; verify one full-grade run before any scored draw. Any draw where personal grades <60 claims is invalid.
4. (S) Chronic-subset manifests: eval/e04_chronic_rupture_cases.json (ruptureops-archive-import-reconciliation, ruptureops-flowworks-campaign-revision, ruptureops-spatial-evidence, ruptureops-forked-agent-idempotency — from eval/rupture_ops_cases.json) and eval/e04_chronic_guard_cases.json (star-rupture-plan-revision, warmind-parser-learning — from eval/work_cases.json).
5. (S) Exact-value claim-slot tagging: annotate rubric slots requiring verbatim values (dates, IDs, numbers, paths) in the three manifests; grader carries the tag into results (pointer-demotion risk per D01).
6. (S) Verify agent_work_eval.py emits per-case `response_character_metrics` service chars; add if missing.
7. (S) Accepted-source-in-context checker: script comparing saved per-case returned context against rubric accepted-source lists (reproduces the 21/22 baseline measurement).

## Arms

- **A** — `service_api`, all D01 flags off (current caps). Paired baseline.
- **B** — `service_api`, `fair_share`=on, `char_cap`=on (`section_demotion_top_n`=8), `top1_hydration`=off.
- **C** — `service_api`, all three flags on (B plus top-1 hydration).
- **F** — `filesystem` reference (instruction-restricted read-only; the writable-sidecar control does not exist yet and is out of scope here).

## Corpus and fixtures

Same corpus and fixtures as the 57-case strict-draw runs, exact+lexical only with embeddings pending — the identical configuration behind every recorded quality and latency baseline. Suites: rupture_ops (primary, 12 cases/48 claims), work (guard, 13/52 per the settled suite-size record; note eval/work_cases.json holds 14 cases/56 claims on disk as of 2026-07-27 — the run uses the manifest as-is, so cost arithmetic below carries the on-disk count), personal_coordination (guard, 15/60) — 41 cases/164 claims per arm-draw at the on-disk count (40/160 at the settled record). The recent suite is excluded: no D01-specific hypothesis, and cost control. Preflight must recount every manifest and use the on-disk counts in the cost ledger.

## Procedure

1. Preflight: clean git tree (implementation fingerprint requires it). Validate every manifest: `python agent_work_eval.py validate --manifest eval/rupture_ops_cases.json` (repeat for eval/work_cases.json, eval/personal_coordination_cases.json, and both chronic manifests). Confirm precondition 3's clean personal run exists.
2. Confirm reasoning runs on the ChatGPT-authenticated Codex subscription; `require_codex_subscription` must reject API keys (fail-closed).
3. For draw N in 1..3, complete all four arms before starting draw N+1. Per arm: set that arm's runtime flags on the API under test (runtime config, no deploy), then per suite run:
   `python agent_work_eval.py run --manifest eval/rupture_ops_cases.json --condition service_api --concurrency 3 --timeout 360 --run-id e04-armA-rupture-draw1 --out results/2026-MM-DD-e04-armA-rupture-draw1.json --report results/2026-MM-DD-e04-armA-rupture-draw1.md`
   Substitute armB/armC and suites work/personal; arm F uses `--condition filesystem`. Artifact naming: `results/2026-MM-DD-e04-<arm>-<suite>-draw<N>.json`.
4. Draws 4-5, chronic subsets only, all four arms: same commands with `--manifest eval/e04_chronic_rupture_cases.json` and `--manifest eval/e04_chronic_guard_cases.json`. Combined with draws 1-3 this yields 5 paired draws for the six chronic cases.
5. Aggregate: `python eval/aggregate_draws.py results/2026-MM-DD-e04-*.json --out results/2026-MM-DD-e04-aggregate.json` — win/loss/tie, McNemar, bootstrap CIs, chars/case per arm.
6. Winning-config soak: `python performance_eval.py run --label e04-winning-soak --future-soak --out results/2026-MM-DD-e04-winning-soak.json` (30 samples, definitive) with that config's flags on.
7. On regression only: one-factor-at-a-time bisection of the three bounds before abandoning D01 — `char_cap` only; `fair_share` only; demotion isolated via `section_demotion_top_n` unset vs 8 — chronic subsets, 3 draws per configuration.

## Metrics

- Per-case claim scores, per suite, per arm, per draw.
- Paired exact-binomial McNemar at case level across draws: B vs A, C vs A, C vs B, and each service arm vs F (vs files).
- Service chars/case via `response_character_metrics` (baselines: RuptureOps ~70,814 simplified vs 41,441 legacy).
- Exact-value claim slots scored separately from other slots.
- Accepted-source-in-context rate on disputed/missed claims (baseline 21/22).
- Soak latency and gate metrics for the winning config.

## Acceptance criteria

Accept D01 (config = best of B/C) only if ALL hold:

1. **RuptureOps primary:** McNemar-significant improvement vs arm A (p<0.05), OR no significant change AND ≥25% reduction in service chars/case (from ~70,814 to ≤ ~53,100).
2. **No guard regression:** no McNemar-significant regression vs arm A on work or personal; personal graded 60/60 in every counted draw.
3. **Chronic set net-positive:** across the six chronic cases over 5 paired draws, wins > losses vs arm A, and no previously-nonzero case falls to 0/5.
4. **640K soak unchanged:** all performance_eval.py gates pass with the winning flags; no drift vs v8 baseline search p95 53.1ms / open 59.7ms (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json); D01's round-trip assertion holds.

Single-draw deltas decide nothing: the noise floor is ±3-5 claims (agent-work native 40→47→44→43→47 across builds). If C fails but B passes, ship B and park `top1_hydration`. OWNER DECISION: whether a B-only pass justifies re-running arm C after D02 (D02-verbatim-span-contract.md) lands.

## Cost preflight and ceiling

Subscription rule (Decisions.md): all reasoning runs on the ChatGPT-authenticated Codex subscription, fail-closed in code (`require_codex_subscription` rejects API keys); observed all-in cost ≈$0.24/agent-run (470-run audit, $113.18).

- Full draws: 4 arms × 41 cases (12+14+15 on-disk; 40 at the settled 13-case work count) × 3 draws = 492 runs × $0.24 = **$118.08** ($115.20 at the settled count).
- Chronic draws 4-5: 4 arms × 6 cases × 2 draws = 48 runs × $0.24 = **$11.52**
- Contingency (personal re-grade/re-run plus bisection: 3 configs × 6 cases × 3 draws = 54 runs): ≤ **$12.96**
- Planned ≈ $129.60; with full contingency ≈ $142.56. **Hard ceiling: $150.**

Embeddings-exempt spend (usage-billed OpenAI, listed separately): **$0 planned** — all arms run exact+lexical with embeddings pending, matching every baseline. For reference, indexing would cost ~$0.19 per 9.6M-token corpus; it is not part of this experiment. Soak and aggregator runs use no reasoning model: $0.

## Abort criteria

- Cost ledger reaches $150: stop, report partial results.
- Any reasoning spend billed to an API key (subscription check bypassed): abort immediately; restore the fail-closed path before resuming.
- Guard catastrophic pattern: any guard case at 0 in ≥2 draws of one arm (the v6 Star Rupture 0/3 shape) → halt that arm, go directly to Procedure step 7 bisection.
- Harness instability: >10% case timeouts in a draw → invalidate the draw, fix, re-run it (charges contingency).
- Soak gate failure in the winning config → D01 rejected regardless of quality results.
- Any checkpoint-lineage incident on Nyx during runs → stop (D14 tripwire class, D14-migration-and-authority-tiers.md).

## Reporting

The run record (`results/2026-MM-DD-e04-report.md`) must contain: git commit fingerprint (clean tree); exact flag/knob configuration per arm; per-suite per-case claim tables for every draw; aggregator output (win/loss/tie counts, McNemar p-values, bootstrap CIs) for every comparison listed in Metrics; chars/case distribution per arm with deltas vs the 70,814/41,441 baselines; the exact-value slot subtable; accepted-source-in-context rate per arm; personal 60/60 grade-completeness per draw; the cost ledger (runs × $0.24 vs ceiling, embeddings spend listed separately); the soak artifact path; and the accept/bisect/abandon decision naming the specific criterion that decided it.

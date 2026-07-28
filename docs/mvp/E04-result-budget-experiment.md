# E04 — Result Budget Experiment

Status: Specified — not run
Date: 2026-07-27
Gates: D01 (D01-budget-contracted-retrieval.md)
Phase: 1 (requires flagged feature build)

## Question

Does budget-contracted retrieval — fair-share allocation + token-tied char cap + section demotion, with and without top-1 complete-artifact hydration — improve RuptureOps answer quality, or hold quality while cutting service chars/case by ≥25%, without regressing the guard suites, under n≥3 paired draws with McNemar?

## Preconditions and build items

1. (M) D01 feature behind flags `search.fair_share` / `search.top1_hydration` / `search.char_cap` in apps/api/src/simple_core.rs: response-assembly budget allocation; hydration reusing open's complete-source path (`MAX_OPEN_COMPLETE_SOURCE_CHARS`); single batched hydration fetch; `section_demotion_top_n` knob.
2. (implemented) Arm-aware n≥3 aggregator and immutable arm/draw ledger binding — see [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
3. (S) Personal suite clean 60-claim run: the last run graded only 40/60.
   Diagnose the saved answers with
   `python3 agent_work_eval.py --manifest eval/personal_coordination_cases.json regrade --input "$INPUT" --out "$OUTPUT"`;
   verify one full-grade run before any scored draw. Any draw where personal
   grades <60 claims is invalid.
4. (S) Chronic-subset manifests: eval/e04_chronic_rupture_cases.json (ruptureops-archive-import-reconciliation, ruptureops-flowworks-campaign-revision, ruptureops-spatial-evidence, ruptureops-forked-agent-idempotency — from eval/rupture_ops_cases.json) and eval/e04_chronic_guard_cases.json (star-rupture-plan-revision, warmind-parser-learning — from eval/work_cases.json).
5. (S) Exact-value claim-slot tagging: annotate rubric slots requiring verbatim values (dates, IDs, numbers, paths) in the three manifests; grader carries the tag into results (pointer-demotion risk per D01).
6. (S) Verify agent_work_eval.py emits per-case `response_character_metrics` service chars; add if missing.
7. (implemented) `eval/audit_accepted_sources.py`, comparing saved `service_operations[].source_paths` against each rubric's accepted sources and emitting `straylight-accepted-source-context-audit@v1`.

## Arms

- **A** — `service_api`, all D01 flags off (current caps). Paired baseline.
- **B** — `service_api`, `fair_share`=on, `char_cap`=on (`section_demotion_top_n`=8), `top1_hydration`=off.
- **C** — `service_api`, all three flags on (B plus top-1 hydration).
- **F** — `filesystem` reference (instruction-restricted read-only; the available writable-sidecar control remains out of scope for this D01-specific experiment).

## Corpus and fixtures

Same corpus and fixtures as the strict-draw runs, exact+lexical only with
embeddings pending. Suites: rupture_ops (primary, 12 cases/48 claims), work
(guard, 13/52 active), and personal_coordination (guard, 15/60) — **40
cases/160 claims per arm-draw**. The recent suite is excluded: no D01-specific
hypothesis, and cost control. Preflight must recount every manifest and use the
on-disk counts in the cost ledger.

## Procedure

1. Preflight: use the isolated Nyx preamble, verify the clean revision, and
   validate every manifest. For the personal grading repair use the correct
   global manifest position:
   `python3 agent_work_eval.py --manifest eval/personal_coordination_cases.json regrade --input "$PERSONAL_INPUT" --out "$PERSONAL_REGRADE"`.
   Confirm a complete 60-claim personal artifact before any scored draw.
2. Before reasoning, run definitive 640K guards for both candidate service
   configurations, one performance stack at a time. The B command is:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile default-safe --label e04-B-soak --future-soak --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=on --expect-feature-flag search_fair_share=on --expect-feature-flag search_char_cap=on --expect-feature-flag search_top1_hydration=off --expect-runtime-config search_section_demotion_top_n=8 --out results/2026-MM-DD-e04-B-soak.json`.
   Run C against its isolated stack by changing hydration to `on`, the label,
   and output. Any red deterministic gate stops all E04 reasoning.
3. Confirm reasoning runs on the ChatGPT-authenticated Codex subscription;
   `require_codex_subscription` must reject API keys.
4. For draw N in 1..3, complete all four arms before starting draw N+1. Per arm, use a unique `--run-id`, stable `--experiment-arm e04-A|B|C|F`, and one shared `--paired-draw-id e04-<suite>-draw<N>` per suite/draw. Example control:
   `python3 agent_work_eval.py --manifest eval/rupture_ops_cases.json run --condition service_api --service-protocol simple --experiment-arm e04-A --paired-draw-id "e04-rupture-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag search_fair_share=off --expect-feature-flag search_top1_hydration=off --expect-feature-flag search_char_cap=off --expect-runtime-config search_section_demotion_top_n=null --concurrency 3 --timeout 360 --run-id "e04-A-rupture-run${N}" --out "results/2026-MM-DD-e04-A-rupture-draw${N}.json" --report "results/2026-MM-DD-e04-A-rupture-draw${N}.md"`
   Substitute B/C and suites work/personal with their exact expected flag/knob values. Arm F uses `--condition filesystem --experiment-arm e04-F` with the same paired-draw ID. Artifact naming: `results/2026-MM-DD-e04-<arm>-<suite>-draw<N>.json`.
5. Draws 4-5, chronic subsets only, all four arms: retain each parent manifest fingerprint and select the frozen IDs with repeated `--case` options. For rupture use `--manifest eval/rupture_ops_cases.json` with the four IDs listed in `eval/e04_chronic_rupture_cases.json`; for work use `--manifest eval/work_cases.json` with the two IDs in `eval/e04_chronic_guard_cases.json`. Combined with draws 1-3 this yields 5 paired draws for the six chronic cases. Do not run the separately fingerprinted subset manifests as aggregate inputs.
6. Build exact input arrays:
   `E04_FULL=(results/2026-MM-DD-e04-{A,B,C,F}-{rupture,work,personal}-draw{1,2,3}.json); E04_CHRONIC=(results/2026-MM-DD-e04-{A,B,C,F}-{rupture,work}-draw{4,5}.json); E04_ALL=("${E04_FULL[@]}" "${E04_CHRONIC[@]}")`.
   Aggregate with
   `python3 eval/aggregate_draws.py "${E04_ALL[@]}" --expected-arm e04-A --expected-arm e04-B --expected-arm e04-C --expected-arm e04-F --allow-case-extension --out results/2026-MM-DD-e04-aggregate.json`.
   Audit service artifacts only:
   `E04_SERVICE=(results/2026-MM-DD-e04-{A,B,C}-{rupture,work,personal}-draw{1,2,3}.json results/2026-MM-DD-e04-{A,B,C}-{rupture,work}-draw{4,5}.json); python3 eval/audit_accepted_sources.py "${E04_SERVICE[@]}" --out results/2026-MM-DD-e04-accepted-source-context.json`.
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

- Full draws: 4 arms × 40 cases × 3 draws = 480 runs × $0.24 = **$115.20**.
- Chronic draws 4-5: 4 arms × 6 cases × 2 draws = 48 runs × $0.24 = **$11.52**
- Contingency (personal re-grade/re-run plus bisection: 3 configs × 6 cases × 3 draws = 54 runs): ≤ **$12.96**
- Planned ≈ $126.72; with full contingency ≈ $139.68. **Hard ceiling: $150.**

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

# E05 — Lexical Consolidation Guard

Status: Executable prerequisites implemented — not run
Date: 2026-07-27
Gates: D10 (D10-read-path-roundtrip-reductions.md) — the deferred lexical-scan consolidation ONLY; the rest of D10 does not wait on this
Phase: 1 (requires flagged feature build)

## Question

Can the up-to-3 sequential lexical candidate scans in the search path be consolidated into a single scan with zero reasoning-quality cost? The bar is "provably free or drop it." This is the exact change class that produced the v6 recent-first collapse (Star Rupture 0/3 — older authoritative sources hidden by a plausible efficiency change), and migration 0055's bounded full-index fallback ("a sparse recent match is only a lead") is the scar tissue. A round-trip saving measured in single-digit milliseconds (search p95 is already 53.1ms at 640K) buys nothing if it costs one claim.

## Preconditions and build items

1. **(M) Flagged implementation** — `lexical_single_scan` runtime flag in `apps/api/src/simple_core.rs`, consolidating the sequential lexical candidate scans into one statement while preserving the recent-first two-tier window (256 recent entries, bounded full-index fallback per migration 0055), scoring (3.0 + ts_rank_cd + term bonus, cap 8.0, − derived_penalty), and all caps. Flag off = current triple scan, byte-for-byte.
2. **(implemented) n≥3 aggregator** — arm-aware pairing, authenticated runtime snapshots, immutable run identity, per-case win/loss/tie, exact-binomial McNemar, and case-level bootstrap CIs are shared through [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
3. **(implemented) Targeted manifest** — `eval/e05_targeted_cases.json` containing only `star-rupture-plan-revision` and `warmind-parser-learning`, same schema as `eval/work_cases.json`, for the 5-draw repeats.
4. **(exists) Deterministic guards** — shipped since v7 in performance_eval:
   640K `old_relevant_source_survives_many_newer_writes` (the result samples
   are stored under `old_source_found`; must be 30/30) and
   `bounded_lexical_overflow_returns_late_relevant_source`.
5. **(implemented) Reproducible arm state** — both evaluators accept
   repeatable `--expect-feature-flag NAME=on|off` and `--run-tag TAG`
   arguments and
   record them in the JSON artifact. Feature-state declarations are checked
   against `/v1/status` before measurements begin; they describe rather than
   mutate the already configured API.
6. **(implemented) Paired query-count comparator** —
   `eval/compare_query_counts.py` consumes two passing definitive v2
   performance artifacts, requires the same clean source/build/image,
   retrieval modes, scales, fixtures, and named-sample shape, and permits the
   authenticated runtime configurations to differ only in one explicitly
   declared false→true boolean feature.

**Resolved nuisance posture (2026-07-28):** E02 rejected D02, so every E05
service and performance stack must start with
`STRAYLIGHT_VERBATIM_SPANS=false` and every measured service arm must assert
`--expect-feature-flag verbatim_spans=off`. Verbatim spans are not an E05
variable. An E05 pass cannot rehabilitate D02.

## Arms

- **Arm A (control):** `lexical_single_scan` off — current up-to-3 sequential scans.
- **Arm B (treatment):** `lexical_single_scan` on — single consolidated scan.

Identical corpus, identical manifests, identical model (from manifest: gpt-5.6-sol), same git commit, clean tree.

## Corpus and fixtures

- Reasoning: agent-work suite (`eval/work_cases.json`, 13 active cases / 52
  claims) — the suite containing the historical lexical-collapse cases.
- Targeted: `star-rupture-plan-revision` (the v6 0/3 case) and `warmind-parser-learning`, via the targeted manifest.
- Deterministic: 640K future-soak fixture (guards); 3,340-record clean fixture optional for latency comparison against results/2026-07-27-3340-clean-30-sample.json.
- All runs exact+lexical (no semantic profile exists; embeddings pending), which is also the worst case for this change — the lexical lane carries everything.

## Procedure

1. Preflight: use the project-scoped isolated Nyx preamble and a clean git
   tree (implementation fingerprint requires it);
   `python3 agent_work_eval.py --manifest eval/work_cases.json validate` and
   `python3 agent_work_eval.py --manifest eval/e05_targeted_cases.json validate`.
2. Deterministic guards on Arm B first (cheap kill), against a disposable API
   already started with `STRAYLIGHT_LEXICAL_SINGLE_SCAN=true`:
   `python3 performance_eval.py run --label e05-armB-guards --gate-profile e05-lexical-consolidation --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --verbatim-feature-acceptance not-applicable --query-budget-profile not-applicable --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag lexical_single_scan=on --expect-build-revision "$REV" --run-tag E05 --run-tag armB-guards --future-soak --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --out results/2026-MM-DD-e05-armB-guards-soak.json`.
   Require `old_relevant_source_survives_many_newer_writes` 30/30 and
   `bounded_lexical_overflow_returns_late_relevant_source` pass. Any failure →
   drop the consolidation, stop the experiment, skip all reasoning runs.
3. After Arm B passes, run a matched Arm A soak on the isolated flag-off
   stack with the same corpus/sample shape and provenance options:
   `python3 performance_eval.py run --label e05-armA-control --gate-profile e05-lexical-consolidation --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --verbatim-feature-acceptance not-applicable --query-budget-profile not-applicable --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag lexical_single_scan=off --expect-build-revision "$REV" --run-tag E05 --run-tag armA-control --future-soak --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --out results/2026-MM-DD-e05-armA-control-soak.json`.
   This supplies the
   comparative latency and query-count control; it does not weaken the Arm B
   cheap-kill gate. Before any reasoning, compare the matched artifacts:
   `python3 eval/compare_query_counts.py --control results/2026-MM-DD-e05-armA-control-soak.json --treatment results/2026-MM-DD-e05-armB-guards-soak.json --feature lexical_single_scan --operation search --min-delta -2 --max-delta 0 --require-strict-improvement --expected-retrieval-modes exact lexical --out results/2026-MM-DD-e05-query-count-comparison.json`.
   Every paired search sample must stay within `[-2,0]`, at least one must
   strictly decrease, every non-search sample must be unchanged, and the
   comparator must pass before reasoning starts.
4. Paired draws, N = 1..3, alternating arms per draw to avoid drift:
   `python3 agent_work_eval.py --manifest eval/work_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e05-A --paired-draw-id "e05-work-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag lexical_single_scan=off --run-tag E05 --run-tag armA --concurrency 3 --timeout 360 --run-id "e05-armA-draw${N}" --out "results/2026-MM-DD-e05-lexical-armA-draw${N}.json" --report "results/2026-MM-DD-e05-lexical-armA-draw${N}.md"` (flag off), then the same with `--experiment-arm e05-B`, `--expect-feature-flag lexical_single_scan=on`, and an API actually started with the treatment flag on. Both arms retain `--service-retrieval-modes exact lexical`, use `--api-container "$API_CONTAINER"`, use the same paired-draw ID, and use distinct run IDs.
5. Targeted 5-draw repeats, N = 1..5, both arms:
   - Arm A:
     `python3 agent_work_eval.py --manifest eval/e05_targeted_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e05-A --paired-draw-id "e05-targeted-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag lexical_single_scan=off --run-tag E05 --run-tag targeted --run-tag armA --concurrency 3 --timeout 360 --run-id "e05-targeted-armA-draw${N}" --out "results/2026-MM-DD-e05-lexical-targeted-armA-draw${N}.json" --report "results/2026-MM-DD-e05-lexical-targeted-armA-draw${N}.md"`.
   - Arm B:
     `python3 agent_work_eval.py --manifest eval/e05_targeted_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e05-B --paired-draw-id "e05-targeted-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag lexical_single_scan=on --run-tag E05 --run-tag targeted --run-tag armB --concurrency 3 --timeout 360 --run-id "e05-targeted-armB-draw${N}" --out "results/2026-MM-DD-e05-lexical-targeted-armB-draw${N}.json" --report "results/2026-MM-DD-e05-lexical-targeted-armB-draw${N}.md"`.
   Both arms use the same `e05-targeted-draw${N}` paired-draw ID. Do not reuse
   the full-suite artifact names or run IDs.
6. Aggregate exact arrays separately:
   `E05_FULL=(results/2026-MM-DD-e05-lexical-arm{A,B}-draw{1,2,3}.json); python3 eval/aggregate_draws.py "${E05_FULL[@]}" --expected-arm e05-B --expected-arm e05-A --expected-arm-retrieval-modes e05-B=exact,lexical --expected-arm-retrieval-modes e05-A=exact,lexical --out results/2026-MM-DD-e05-full-aggregate.json`.
   The targeted aggregate is a separate command with its frozen five-draw
   filenames:
   `E05_TARGETED=(results/2026-MM-DD-e05-lexical-targeted-arm{A,B}-draw{1,2,3,4,5}.json); python3 eval/aggregate_draws.py "${E05_TARGETED[@]}" --expected-arm e05-B --expected-arm e05-A --expected-arm-retrieval-modes e05-B=exact,lexical --expected-arm-retrieval-modes e05-A=exact,lexical --out results/2026-MM-DD-e05-targeted-aggregate.json`.
   Do not mix the full and targeted case sets.
7. Record per-lane latency metrics and the fail-closed named-sample
   query-count comparison from both arms (the D09 assertion delta this would
   lock in if shipped). Counts come only from the request-scoped D09
   instrumentation; the comparator never infers them from latency or logs.

## Metrics

- Claims scored (x/52) per draw per arm; per-case paired win/loss/tie; exact-binomial McNemar p; bootstrap CI on the case-level difference.
- Targeted cases: pass count out of 5 draws per case per arm.
- Deterministic: guard pass/fail; search p95 and lexical query count per operation, both arms, vs the v8 baseline (search 53.1ms).

## Acceptance criteria

Ship the consolidation only if ALL hold; otherwise drop it permanently and record a negative result (as with v6):

1. Deterministic guards clean:
   `old_relevant_source_survives_many_newer_writes` 30/30 and
   `bounded_lexical_overflow_returns_late_relevant_source` pass with flag on;
   `results/2026-MM-DD-e05-query-count-comparison.json` passes with all search
   deltas in `[-2,0]`, at least one strict reduction, and no non-search change.
2. n≥3 paired agent-work: McNemar shows no significant regression for Arm B (α = 0.05) AND the point estimate of the case-level difference is ≥ 0 or its CI comfortably includes 0. Single-draw deltas are noise (±3–5 claims observed: 40→47→44→43→47) and carry no weight.
3. Targeted 5-draw: Arm B ≥ Arm A on both `star-rupture-plan-revision` and `warmind-parser-learning`; any B < A on `star-rupture-plan-revision` is an automatic drop regardless of aggregate stats.

## Cost preflight and ceiling

Subscription rule: all reasoning runs go through the ChatGPT-authenticated Codex subscription, fail-closed (`require_codex_subscription` rejects API keys). All-in equivalent cost ≈ $0.24/agent-run (470-run audit, $113.18).

- Paired draws: 13 cases × 2 arms × 3 draws = 78 runs.
- Targeted: 2 cases × 2 arms × 5 draws = 20 runs.
- Total 98 runs × $0.24 = $23.52 all-in equivalent.
- Embeddings-exempt spend: $0 — no semantic lane in these runs (embeddings pending across all fixtures).
- Deterministic soak runs: compute-local, no reasoning spend.

Hard ceiling: **$40**. Headroom (~$16) covers at most ~2 rerun draws for infrastructure failures; beyond that, stop and report.

## Abort criteria

- Step 2 guard failure: immediate drop, no reasoning runs (this is the designed cheap exit).
- Projected spend exceeding $40 at any point.
- More than 2 case-level harness failures (timeouts/errors) in a draw: draw invalid, rerun once; a second invalid draw for the same arm aborts the experiment.
- Any soak regression in untouched paths — concurrent-write p95 above 2x the v8 baseline (29.0ms) with the flag on: abort and investigate (the write path regressed twice in one day historically; a lexical change has no business moving it).
- Dirty git tree or fingerprint mismatch between arms: results void, restart.

## Reporting

The run record must contain: all artifact paths
(`results/2026-MM-DD-e05-lexical-arm{A,B}-draw{1,2,3}.json`,
`results/2026-MM-DD-e05-lexical-targeted-arm{A,B}-draw{1,2,3,4,5}.json`,
and the soak JSONs); both aggregate artifacts (full and targeted);
win/loss/tie tables, McNemar p, and bootstrap CIs; guard results verbatim;
per-lane latency and the paired named-sample query-count artifact/deltas; git commit fingerprint and flag states
per run; actual spend vs the $23.52 preflight; and a one-line verdict —
"provably free: ship behind `lexical_single_scan` per D10" or "not free:
dropped, negative result recorded alongside v6 recent-first." If dropped,
D10's deferred item is closed, not merely postponed.

## References

- D10-read-path-roundtrip-reductions.md (the gated design item); D09-latency-contract-and-gates.md (query-count assertions that would lock the win).
- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json; results/2026-07-27-3340-clean-30-sample.json; v5/v7 future-soak JSONs.
- Vault: v6 recent-first negative experiment (Star Rupture 0/3); migration 0055 rationale; Decisions.md (cost rules, subscription fail-closed).

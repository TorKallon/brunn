# E10 — Combined Preflight

Status: Deterministic D09 preflight implemented — not run
Date: 2026-07-27
Gates: D01, D02, D03, D04, D05 in combination — the final pre-launch gate before Tier C sole authority (D14-migration-and-authority-tiers.md, which lists E10 in gate 5 and the Tier C entry requirements); no cutover proceeds without it
Phase: 1 (requires flagged feature build — all shipped Dxx flags landed)

## Question

With ALL shipped flags on simultaneously, under one global budget, is the simplified core non-inferior to filesystem+sidecar everywhere and superior somewhere — at n≥3, with every deterministic gate green at 64K and 640K?

Each of D01-D05 ships behind its own runtime flag and passes its own paired-draw experiment in isolation. That is necessary and insufficient. The 2026-07-26 production collapse was not one large mistake; it was the accretion of individually-small synchronous costs that no single test gated. The 2026-07-22 cross-query dedup experiment passed local reasoning ("less redundant context must be fine") and hurt quality; v6's recent-first lexical passed its own latency profile and hid older authoritative sources (Star Rupture 0/3). Individually-bounded changes compose into unbounded systems. E10 measures the composition itself, once, under the exact configuration that would ship.

This is the "stronger than MD" endgame gate: the claim was never that a database reasons better than Markdown, but launch requires the combined system to beat the honest file baseline somewhere while losing nowhere.

## Preconditions and build items

1. All of D01-D05 landed as shipped (flags on by default or explicitly enabled), each having individually passed its own Exx gate. Any Dxx that did not ship is simply absent from the flag set — E10 tests what will actually launch, and the run record says exactly what that was.
2. E01 machinery complete (E01-paired-draw-machinery-and-baseline.md): eval/aggregate_draws.py, `filesystem_sidecar` condition in agent_work_eval.py, and — required by this point, per the E01 deferral — the transitions sidecar seed extension (Medium) so the control arm covers all five suites.
3. One global budget: the combined context/char budget configuration (per D01) active as a single runtime config, not per-feature budgets summed implicitly. Target posture per Tier B: crude open/search char budget near legacy ~41.4K chars/case at entry.
4. Flag-manifest snapshot (Small): the harness records the exact runtime config (every flag name and value, git SHA, clean-tree fingerprint) into each run artifact. A combined verdict is meaningless without knowing precisely which composition was measured.
5. D09 (D09-latency-contract-and-gates.md) gates wired and green individually before this run; E03 semantic posture decided by E09 and reflected in the flag manifest (semantic remains off the Tier B critical path regardless).

## Arms

- **A: simplified `service_api`, all shipped flags ON, one global budget.** The launch configuration, bit-for-bit.
- **B: `filesystem_sidecar`.** Writable sidecar beside the read-only corpus symlink, checkpoint file required — the fair, un-handicapped file baseline.

Two arms only; this is a launch gate, not a factorial study. Per-flag attribution already happened in each Dxx's own experiment.

## Corpus and fixtures

Full 5-suite matrix, 57 cases / 228 claims (agent-work 13/52, recent 12/48, rupture 12/48, personal 15/60, transitions 5/20). n≥3 paired draws, identical fixtures across draws and arms. Deterministic gates run on the standard performance corpora: 64K default scale and 640K `--future-soak`.

## Procedure

MM-DD is the run date.

1. Freeze the launch flag set; record the flag manifest; verify clean git tree.
2. Deterministic pass first (cheap, fails fast), against the exact isolated API image and database container: `python performance_eval.py run --protocol simple --label e10-perf-64k --scales 64000 --samples 30 --api-container <api> --db-container <db> --out results/2026-MM-DD-e10-perf-64k.json` with all flags on — all D09 gates (regression tier, query-count budgets vs `eval/query_budgets.json`, app-role/function-body EXPLAIN plan assertions, SQL-drift fingerprints, phase-sum sanity), then `python performance_eval.py run --protocol simple --label e10-perf-640k --future-soak --api-container <api> --db-container <db> --out results/2026-MM-DD-e10-perf-640k.json` including the concurrent write/search probe, semantic-failure probe, checkpoint footprint (≤100 rows/4MiB), protocol-to-evidence ratio ≤1.0, and flat-file control. Supply both semantic-failure hooks required by the harness. Any red gate stops the experiment before a single reasoning dollar is spent. The first D09-enabled run must also confirm that the checked-in code-shape query budgets are the observed default-safe counts before they are described as measured baselines.
3. For each draw N in 1..3, for each manifest M in {work, recent_work, rupture_ops, personal_coordination}:
   `python agent_work_eval.py run --manifest eval/M_cases.json --condition service_api --condition filesystem_sidecar --concurrency 3 --timeout 360 --run-id e10-M-draw<N> --out results/2026-MM-DD-e10-M-draw<N>.json --report results/2026-MM-DD-e10-M-draw<N>.md` (harnesses live at repo root; eval/ holds only manifests)
4. For each draw N: `python transition_eval.py run --condition service_api_resume --condition filesystem_sidecar --embeddings none --run-id e10-transitions-draw<N> --out results/2026-MM-DD-e10-transitions-draw<N>.json --report results/2026-MM-DD-e10-transitions-draw<N>.md` (condition name per the E01 build item's final spelling).
5. Scoring iteration via regrade only; no regeneration to chase rubric issues.
6. Aggregate: `python eval/aggregate_draws.py results/2026-MM-DD-e10-*-draw*.json --out results/2026-MM-DD-e10-aggregate.json`.
7. If corpus-wide non-inferiority holds but no suite reaches superiority, one additional draw (N=4) may be run to resolve it, inside the ceiling; the aggregate then reports n=4 everywhere (no cherry-picking draws).

## Metrics

- Per-suite and corpus-wide paired win/loss/tie; McNemar exact p; case-level bootstrap 95% CIs (E01 machinery).
- Chars/case per arm per suite; ratio A/B.
- All D09 gate outputs at both scales; 640K soak results including concurrent probes.
- Checkpoint accounting both arms (`persisted_checkpoints` vs service checkpoints); chronic-case outcomes (the E01 chronic list) reported for regression visibility.

## Acceptance criteria

All five must hold; any miss means no Tier C cutover:

1. **Non-inferiority everywhere:** for every one of the five suites, McNemar two-sided p ≥ 0.05 against arm A AND the bootstrap 95% CI lower bound on the claim difference is ABOVE the E01 noise-floor margin of −5 claims corpus-wide (matching E01 acceptance criterion 2), with no per-suite CI showing a significant deficit. (A lower bound at or below −5 is the failure condition, not the pass condition.)
2. **Superiority somewhere:** McNemar p < 0.05 in arm A's favor on ≥1 suite.
3. **Context discipline:** corpus-wide chars/case in arm A ≤ arm B + 10%.
4. **All D09 gates green** at 64K with the full flag set: regression-tier latencies, query-count budgets, EXPLAIN plan assertions.
5. **640K soak green:** hard SLOs, concurrent write/search probe, checkpoint footprint, protocol-to-evidence ratio, no latency drift with change-log growth.

## Cost preflight and ceiling

Reasoning runs: 57 cases × 2 conditions × 3 draws = 342 runs × $0.24 (observed all-in equivalent; 470-run audit, $113.18) ≈ **$82.08**. Optional draw 4: +114 runs ≈ $27.36; worst case ≈ $109.44. All reasoning via the ChatGPT-authenticated Codex subscription, fail-closed (`require_codex_subscription` rejects API keys). Deterministic performance runs: $0 reasoning. Embeddings-exempt spend, listed separately: $0 if the corpus is already embedded from prior work; at most one backfill ≈ $0.19-2 (usage-billed OpenAI, exempt per Decisions.md).

Ceiling: **$120** hard. OWNER DECISION: explicit owner approval required before launching the draws (2026-07-27 cost-audit rule); approval of this spec is not approval of the spend.

## Abort criteria

- Any red gate in procedure step 2: stop before reasoning runs begin.
- Any checkpoint-lineage incident in either arm during any draw: immediate abort, Markdown remains authority — this mirrors the Tier C shadow tripwire verbatim and is non-negotiable.
- Cost tracker crosses $120: stop, aggregate complete draws only.
- >10% harness/infra failures in a draw: stop, fix, rerun that draw under a fresh run-id.
- Any flag flipped mid-experiment: the entire experiment restarts — a mixed-configuration aggregate is worthless.

## Reporting

The run record (results/2026-MM-DD-e10-aggregate.json + Markdown) must contain: the flag manifest and git SHA (the measured launch configuration, exactly); all artifact paths; per-suite paired tables with McNemar p and CIs; chars/case table and A/B ratio; complete D09 gate output at 64K and the 640K soak results; checkpoint accounting; chronic-case outcomes; cost actuals vs preflight; and a single yes/no verdict against the five acceptance criteria with the failing criterion named if any. This record is the document the Tier C cutover decision cites.

## References

- 2026-07-26 collapse (accretion of individually-small synchronous costs); 2026-07-22 dedup revert; v6 recent-first collapse (Star Rupture 0/3) — vault incident/experiment notes
- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json (soak baseline the flags-on soak diffs against)
- D01, D02, D03, D04, D05 (the composed flag set); D09-latency-contract-and-gates.md (gates); D14-migration-and-authority-tiers.md (Tier C tripwires this mirrors; lists E10 as a gate)
- E01-paired-draw-machinery-and-baseline.md (machinery + baseline record); E03-semantic-ready-latency-profile.md and E09 (semantic posture in the flag manifest); Decisions.md (cost rules)

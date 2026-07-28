# E10 — Combined Preflight

Status: Prerequisite abort — accepted launch manifest is not qualified
Date: 2026-07-27
Gates: D01, D02, D03, D04, D05 in combination — the final pre-launch gate before Tier C sole authority (D14-migration-and-authority-tiers.md, which lists E10 in gate 5 and the Tier C entry requirements); no cutover proceeds without it
Phase: 1 (requires flagged feature build — all shipped Dxx flags landed)

**CURRENT PREREQUISITE ABORT (2026-07-28):** Do not run E10. The accepted
immutable launch flag manifest is incomplete and not qualified. E01 is
complete, but E02 rejected D02, E05 rejected `lexical_single_scan`, E04 and
E06–E08 have not collectively produced the accepted launch feature set, and
E09 has no decided semantic posture because E03 Mode 2 failed before quality
backfill. This abort does not change E10's future role or experimental intent.
Rebuild the manifest only from accepted features after those gates resolve.

## Question

With ALL shipped flags on simultaneously, under one global budget, is the simplified core non-inferior to filesystem+sidecar everywhere and superior somewhere — at n≥3, with every deterministic gate green at 64K and 640K?

Each of D01-D05 ships behind its own runtime flag and passes its own paired-draw experiment in isolation. That is necessary and insufficient. The 2026-07-26 production collapse was not one large mistake; it was the accretion of individually-small synchronous costs that no single test gated. The 2026-07-22 cross-query dedup experiment passed local reasoning ("less redundant context must be fine") and hurt quality; v6's recent-first lexical passed its own latency profile and hid older authoritative sources (Star Rupture 0/3). Individually-bounded changes compose into unbounded systems. E10 measures the composition itself, once, under the exact configuration that would ship.

This is the "stronger than MD" endgame gate: the claim was never that a database reasons better than Markdown, but launch requires the combined system to beat the honest file baseline somewhere while losing nowhere.

## Preconditions and build items

1. **NOT SATISFIED.** All surviving D01-D05 candidates have a completed ship-or-drop decision from their own Exx gate. Any Dxx that did not ship is absent from the flag set — E10 tests what will actually launch, and the run record says exactly what that was. D02 is currently rejected; E05 separately rejects the deferred D10 `lexical_single_scan` candidate; and E04 plus E06–E08 have not collectively supplied the accepted feature set.
2. **SATISFIED.** E01 machinery is complete (E01-paired-draw-machinery-and-baseline.md): the definitive 531-case-run matrix includes `eval/aggregate_draws.py`, the `filesystem_sidecar` condition, and transitions-sidecar coverage across all five suites.
3. One global budget: the combined context/char budget configuration (per D01) active as a single runtime config, not per-feature budgets summed implicitly. Target posture per Tier B: crude open/search char budget near legacy ~41.4K chars/case at entry.
4. Flag-manifest snapshot (implemented): every service run stores an authenticated `/v1/status` runtime-feature/knob/build snapshot whose canonical hash is bound into the immutable run ledger.
5. **NOT SATISFIED.** D09 (D09-latency-contract-and-gates.md) gates wired and green individually before this run; E03 semantic posture decided by E09 and reflected in the flag manifest (semantic remains off the Tier B critical path regardless). E09 is prerequisite-aborted until E03's failed Mode 2 and unrun quality backfill are repaired.

## Arms

- **A: simplified `service_api`, all shipped flags ON, one global budget.** The launch configuration, bit-for-bit.
- **B: `filesystem_sidecar`.** Writable sidecar beside the read-only corpus symlink, checkpoint file required — the fair, un-handicapped file baseline.

Two arms only; this is a launch gate, not a factorial study. Per-flag attribution already happened in each Dxx's own experiment.

## Corpus and fixtures

Full 5-suite matrix at the checked-in active manifests, 59 cases / 236 claims
(agent-work 13/52, recent 14/56, rupture 12/48, personal 15/60,
transitions 5/20). n≥3 paired draws, identical fixtures across draws and
arms. Deterministic gates run on the standard performance corpora: 64K default
scale and 640K `--future-soak`.

## Procedure

MM-DD is the run date.

1. Use the isolated Nyx preamble and freeze the exact final launch-candidate
   revision—not an earlier experiment
   SHA—and the complete runtime manifest. Use one immutable image across all
   stacks. The current provisional semantic-off posture must already bind the
   two rejected features off; the remaining feature values are illustrative
   until E04 and E06–E09 resolve and therefore do not constitute an accepted
   immutable launch manifest:
   `E10_RUNTIME=(--expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag search_fair_share=on --expect-feature-flag search_top1_hydration=on --expect-feature-flag search_char_cap=on --expect-runtime-config search_section_demotion_top_n=8 --expect-feature-flag verbatim_spans=off --expect-feature-flag resume_deltas=on --expect-feature-flag supersession_demotion=on --expect-runtime-config supersession_demotion_weight=1.5 --expect-feature-flag intention_ledger=on --expect-feature-flag read_path_roundtrip_v1=on --expect-feature-flag lexical_single_scan=off)`.
   Remove any rejected feature from the launch behavior instead of pretending
   it shipped, while still asserting its authenticated off state.
2. The default-safe query contract is forbidden for this combined shape.
   First run explicit count-capture calibrations with
   `--query-budget-profile calibration` for both the launch manifest and the
   otherwise-identical `resume_deltas=off` control. Calibration artifacts
   intentionally fail acceptance. Review their 30-sample counts and freeze two
   runtime-bound contracts as `LAUNCH_QUERY_BUDGET_CONTRACT` and
   `LAUNCH_RESUME_CONTROL_BUDGET_CONTRACT`; absence of either is a hard
   preflight failure. The launch calibration command is:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile calibration --label e10-launch-budget-calibration --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" "${E10_RUNTIME[@]}" --out results/2026-MM-DD-e10-launch-budget-calibration.json`.
   Repeat on the control stack with the same manifest except
   `resume_deltas=off` and a distinct label/output.
3. Produce a passing, same-build 640K resume-off control:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile launch-resume-control --query-budget-contract "$LAUNCH_RESUME_CONTROL_BUDGET_CONTRACT" --exercise-resume-delta-fixture --label e10-resume-control --future-soak --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag resume_deltas=off --expect-feature-flag search_fair_share=on --expect-feature-flag search_top1_hydration=on --expect-feature-flag search_char_cap=on --expect-runtime-config search_section_demotion_top_n=8 --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=on --expect-runtime-config supersession_demotion_weight=1.5 --expect-feature-flag intention_ledger=on --expect-feature-flag read_path_roundtrip_v1=on --expect-feature-flag lexical_single_scan=off --out results/2026-MM-DD-e10-resume-control.json`.
4. Run the cheap launch candidate gates before reasoning. The 64K command is:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile launch --query-budget-contract "$LAUNCH_QUERY_BUDGET_CONTRACT" --label e10-perf-64k --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" "${E10_RUNTIME[@]}" --out results/2026-MM-DD-e10-perf-64k.json`.
   Then run the 640K/D03 form:
   `python3 performance_eval.py run --gate-profile d03-resume-deltas --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile launch --query-budget-contract "$LAUNCH_QUERY_BUDGET_CONTRACT" --resume-control-from results/2026-MM-DD-e10-resume-control.json --exercise-resume-delta-fixture --label e10-perf-640k --future-soak --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" "${E10_RUNTIME[@]}" --out results/2026-MM-DD-e10-perf-640k.json`.
   This runs every generic D09 gate plus D03's ≤150ms and paired exact +5
   completed-statement gate for one authenticated batched version-pair
   `SELECT`. Any red gate stops before reasoning.
5. For each draw `N` in `1 2 3`, invoke each real agent manifest explicitly:
   `python3 agent_work_eval.py --manifest eval/work_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --condition filesystem_sidecar "${E10_RUNTIME[@]}" --concurrency 3 --timeout 360 --run-id "e10-work-draw${N}" --out "results/2026-MM-DD-e10-work-draw${N}.json" --report "results/2026-MM-DD-e10-work-draw${N}.md"`.
   Repeat with manifest/slug pairs `recent_work_cases.json`/`recent`,
   `rupture_ops_cases.json`/`rupture`, and
   `personal_coordination_cases.json`/`personal`.
6. For each draw:
   `python3 transition_eval.py --manifest eval/transition_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api_resume --condition filesystem_sidecar "${E10_RUNTIME[@]}" --embeddings none --run-id "e10-transitions-draw${N}" --out "results/2026-MM-DD-e10-transitions-draw${N}.json" --report "results/2026-MM-DD-e10-transitions-draw${N}.md"`.
7. Scoring iteration uses regrade only with the correct global manifest; no
   regeneration to chase rubric issues.
8. Aggregate only the declared draw artifacts:
   `E10_DRAWS=(results/2026-MM-DD-e10-{work,recent,rupture,personal,transitions}-draw{1,2,3}.json); python3 eval/aggregate_draws.py "${E10_DRAWS[@]}" --expected-arm-retrieval-modes service_api=exact,lexical --expected-arm-retrieval-modes service_api_resume=exact,lexical --out results/2026-MM-DD-e10-aggregate.json`.
9. If corpus-wide non-inferiority holds but no suite reaches superiority, one additional draw (N=4) may be run to resolve it, inside the ceiling; the aggregate then reports n=4 everywhere (no cherry-picking draws).

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

Reasoning runs: 59 cases × 2 conditions × 3 draws = 354 runs × $0.24 =
**$84.96**. Optional draw 4: +118 runs = $28.32; worst case = **$113.28**.
All reasoning uses the ChatGPT-authenticated Codex subscription, fail-closed
(`require_codex_subscription` rejects API keys). Deterministic performance
runs cost $0 reasoning. Under the specified semantic-off launch posture,
embeddings-exempt spend is $0; if E09 changes that posture, this procedure and
its failure-probe mechanism must be revised before execution.

Ceiling: **$120** hard. OWNER DECISION: explicit owner approval required before launching the draws (2026-07-27 cost-audit rule); approval of this spec is not approval of the spend.

## Abort criteria

- Any calibration integrity failure in procedure step 2, or any red
  acceptance-eligible gate in steps 3–4: stop before reasoning runs begin.
  Step 2 calibration artifacts intentionally fail acceptance; that expected
  ineligible verdict is not itself an abort. Missing/inconsistent count
  evidence or failure to freeze either runtime-bound contract is.
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

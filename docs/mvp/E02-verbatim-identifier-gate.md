# E02 — Verbatim Identifier Gate

Status: Definitive failure — Stage 1 confirmed the defect; Stage 2 flag-on failed 4/30 at every scale; soak and reasoning aborted
Date: 2026-07-28
Gates: D02 (D02-verbatim-span-contract.md)
Phase: 0 (stage 1, harness-only, pre-code) then 1 (stage 2, requires D02 built behind verbatim_spans)

## Question

Does the search payload return literal identifier lines verbatim when the matching line lies beyond the 2,400-char excerpt window — and does D02's verbatim_matches close the documented paraphrase/exact-value loss without payload bloat or reasoning regression?

Stage 1 exists to document the defect on the current build before any code changes. Stage 2 gates the fix. Both stages become permanent harness gates.

## Definitive result

**E02 rejects the current D02 implementation.** Stage 1 confirmed the original
defect at 0/30 for 1K, 10K, and 64K. The Stage 2 flag-off control reproduced
0/30 at all three scales. Flag-on improved each scale to only 4/30, far below
the blocking 30/30 gate. Every successful flag-on probe was planted at byte
offset 2,600; no deeper planted identifier survived into the source payload.

| Arm | 1K returned / p95 | 10K returned / p95 | 64K returned / p95 | Result |
| --- | ---: | ---: | ---: | --- |
| Stage 1, pre-D02 | 0/30 / 10.963ms | 0/30 / 34.738ms | 0/30 / 20.081ms | Expected defect confirmed |
| Stage 2, flag off | 0/30 / 10.828ms | 0/30 / 32.673ms | 0/30 / 14.056ms | Expected control confirmed |
| Stage 2, flag on | 4/30 / 9.463ms | 4/30 / 20.558ms | 4/30 / 53.821ms | **FAIL** |

The formal calibration reproduced checkpoint/open/read/resume/search/write
query counts of 28/17/11/32/11/14. Both Stage 2 arms retained those counts, so
flag-on added no SQL round trip. All latency gates passed. The largest
identifier-probe response was 5,761 characters, below the 9,600-character
feature cap. Revision, image, runtime-flag, retrieval-plan, sample-count, and
clean-source gates also passed. The only acceptance failure in each Stage 2
arm was `verbatim_identifier`; flag-off failure is the expected control, while
flag-on failure rejects the feature.

Per the abort criteria, the 640K soak and all reasoning draws were not run.
This avoided 134 unnecessary case-runs and the $32.16 subscription-equivalent
preflight. Actual reasoning API and embedding API cost were both **$0**.

Keep `verbatim_spans` default-off. Repair the feature contract, then rerun the
free deterministic flag-on arm before authorizing either the soak or reasoning
draws.

Immutable evidence:

- [Stage 1](../../results/2026-07-27-verbatim-identifier-stage1.json),
  SHA-256
  `cec7a52f21e2df0a66e7c0aa0e05d71daaafe8cc868c3fd401076857a5cbb56a`
- [Stage 2 calibration](../../results/2026-07-27-verbatim-identifier-stage2-calibration.json),
  SHA-256
  `e9ac26e2e0cf07aad19e0ac0371520e198165daf8a22c328f661230436e23877`
- [Stage 2 flag off](../../results/2026-07-27-verbatim-identifier-stage2-off.json),
  SHA-256
  `437289212bd175bd48ee2cc8ecc8c0e2be6f5c7737b179ed7c9c2b771c702351`
- [Stage 2 flag on](../../results/2026-07-27-verbatim-identifier-stage2-on.json),
  SHA-256
  `781dc82126da533328b838caefe3562c746d4ced1b0ef01e54dbed4346492384`
- [Compact definitive summary](../../results/2026-07-27-e02-definitive-summary.json)

## Preconditions and build items

- B1 (S): Extend performance_eval.py synthetic_documents generation to plant a unique identifier token (format `STRAYID-<scale>-<n>-<hex8>`) at a UTF-8 byte offset strictly greater than 2,400 into 30 selected documents per scale, recording (path, identifier, byte offset, position, section depth) in the fixture manifest. Anchor: performance_eval.py synthetic corpus generator.
- B2 (S): Verbatim-return checker: for each planted identifier, issue an exact-only search whose query contains the exact path plus the identifier and assert the identifier appears in a source-text field of the raw search response. Request/query echoes never count, and follow-up open/read does not count. Register this as the named blocking gate `verbatim_identifier`. This isolates the exact-lane 2,400-char excerpt defect; allowing lexical mode would trivially retrieve the planted identifier and invalidate stage 1.
- B3 (M, stage 2 only): D02 built behind verbatim_spans — exact-lane response assembly in apps/api/src/simple_core.rs plus byte-for-byte passthrough in apps/mcp memory.query.
- B4 (implemented): arm-aware n≥3 aggregator in `eval/aggregate_draws.py`; separate service invocations use the immutable arm/draw contract in [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
- B5 (implemented): `identifier_heavy` tag in `eval/recent_work_cases.json` plus frozen `eval/e02_identifier_cases.json`: recent-aether-gmail-actions, recent-europe-calendar-dedup, recent-aether-morning-brief, recent-tracker-no-delta, and recent-tracker-material-delta (5 cases / 20 claims).

Stage 1 requires only B1+B2. Embeddings are unnecessary: the probe is exact-lane, and all latency baselines are exact+lexical; use tests/mock_openai_embeddings.py only if a run insists on semantic coverage.

## Arms

- Stage 1 (Phase 0): single arm — current build, no flag. Expected result: FAIL (0/30 identifiers verbatim in-payload per scale). The failing artifact is the defect record cited by D02.
- Stage 2 deterministic: verbatim_spans off vs on; 30 planted identifiers per scale.
- Stage 2 reasoning: service_api with flag off vs service_api with flag on, recent-work manifest, n≥3 paired draws; optional targeted 5-draw repeat on the identifier-heavy tag if the headline delta sits inside the ±3-5 claim noise floor.

## Corpus and fixtures

- Synthetic performance corpus at default scales 1k/10k/64k plus 640k via --future-soak; 30 planted identifiers per scale, all past UTF-8 byte 2,400, mixed positions (mid-document and tail) and mixed section depths.
- Reasoning: the recent-work fixture corpus used by agent_work_eval (14 cases / 56 claims), model gpt-5.6-sol from manifest; targeted repeat uses the frozen 5-case / 20-claim E02 manifest.

## Procedure

1. Land B1+B2 on a clean git tree and use the project-scoped isolated Nyx
   preamble in
   [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
   Resolve `API_CONTAINER` and `DB_CONTAINER` through that exact Compose
   project, never through bare `docker compose`.
2. These runs deliberately request only exact+lexical retrieval from a
   semantic-disabled stack. This is a non-default query shape, so it may not
   inherit the global default-safe query budget:
   `E02_PERF=(--protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile e02-verbatim --query-budget-contract eval/query_budgets.e02-verbatim.json --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off)`.
3. Stage 1 run at default scales and 30 samples:
   `python3 performance_eval.py run "${E02_PERF[@]}" --expect-feature-flag verbatim_spans=off --label verbatim-identifier-stage1 --out results/2026-MM-DD-verbatim-identifier-stage1.json`.
   The command is expected to exit nonzero because `verbatim_identifier` is a blocking known-failing gate before D02. `--quick` is acceptable only for smoke.
4. Record the expected failure per scale (target: 0/30 exact-only source payloads). If any identifier does return verbatim, record its position and response field and re-scope D02 before proceeding.
5. Keep `verbatim_identifier` blocking but document the stage-1 failure until D02 ships. Stage 1 is complete; no model runs occurred.
6. Stage 2 begins only after B3 lands on a clean tree.
7. Before either Stage-2 arm, capture the exact shape with
   `E02_CALIBRATION=(--protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile calibration --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off)` and run
   `python3 performance_eval.py run "${E02_CALIBRATION[@]}" --label verbatim-identifier-stage2-calibration --out results/2026-MM-DD-verbatim-identifier-stage2-calibration.json`.
   Calibration is intentionally ineligible for acceptance. Review its
   per-operation counts against
   `eval/query_budgets.e02-verbatim.json`; any difference stops E02 until the
   code shape and contract are reconciled. The contract binds exact+lexical,
   semantic-off retrieval and declares `verbatim_spans` as the sole experiment
   variable, so both arms must use the same reviewed limits.
8. After the calibration exactly matches its reviewed contract, run distinct
   isolated flag-off and flag-on stacks with the corresponding authenticated
   `--expect-feature-flag verbatim_spans=off|on`. Use the E02 performance
   array above for `verbatim-identifier-stage2-off` and
   `verbatim-identifier-stage2-on`; add `--future-soak` to the flag-on
   `verbatim-identifier-stage2-soak`. Flag off must preserve the failure; flag
   on must reach 30/30 at every scale.
9. Reasoning pass, paired draws. For `N` in `1 2 3`, flag off:
   `python3 agent_work_eval.py --manifest eval/recent_work_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm verbatim-off --paired-draw-id "verbatim-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --concurrency 3 --timeout 360 --run-id "verbatim-off-run${N}" --out "results/2026-MM-DD-verbatim-off-draw${N}.json" --report "results/2026-MM-DD-verbatim-off-draw${N}.md"`.
   Run the same draw against the isolated flag-on stack with
   `--experiment-arm verbatim-on`,
   `--expect-feature-flag verbatim_spans=on`, and unique on-arm paths.
10. Aggregate only the six declared full-draw artifacts:
   `VERBATIM_FULL=(results/2026-MM-DD-verbatim-{on,off}-draw{1,2,3}.json); python3 eval/aggregate_draws.py "${VERBATIM_FULL[@]}" --expected-arm verbatim-on --expected-arm verbatim-off --expected-arm-retrieval-modes verbatim-on=exact,lexical --expected-arm-retrieval-modes verbatim-off=exact,lexical --out results/2026-MM-DD-verbatim-aggregate.json`.
   Repeated draws remain clustered by case; McNemar is separate from the
   claim-difference bootstrap.
11. If overall delta is inside the noise floor but identifier-tagged cases move, repeat step 9 for 5 draws with `--manifest eval/e02_identifier_cases.json`, using targeted-specific run and paired-draw IDs, and aggregate separately.
12. Use `agent_work_eval.py regrade` for any rubric corrections; never regenerate answers to fix grading.
13. Promote `verbatim_identifier` to a permanent, blocking performance_eval gate: with flag on it fails closed below 30/30; with flag off it remains a documented expected-fail.

## Metrics

- Verbatim in-payload rate, n/30 per scale, per flag state.
- Search p95 flag on vs off at each scale, vs the 53.1ms v8 640K baseline and the ≤3,000ms hard gate.
- Payload chars/case delta (overfetch guard; reference legacy ~41,441 vs RuptureOps ~70,814 service chars/case) and verbatim payload chars vs the 9,600-char cap.
- Claims per draw (n/56 full; n/20 targeted), per-case win/loss/tie, exact McNemar p; identifier-tagged case wins.
- Query count per search (must equal flag-off count exactly).

## Acceptance criteria

- Stage 1: defect documented — 0/30 (or the measured near-zero rate) recorded in results/, cited from D02.
- Stage 2 deterministic: 30/30 at all scales including the 640k soak; search p95 within hard gate and no drift beyond run noise vs 53.1ms; zero added round trips; payload growth within D02 caps.
- Stage 2 reasoning: across n≥3 paired draws, no significant overall regression (exact McNemar, alpha 0.05) and net positive case wins on the identifier-heavy tag; direction consistent across draws. Single-draw deltas are non-load-bearing per the ±3-5 claim noise floor.
- MCP passthrough equality test green.

## Cost preflight and ceiling

Subscription rule: all reasoning runs via ChatGPT-authenticated Codex subscription, fail-closed (require_codex_subscription rejects API keys). Usage-billed OpenAI is permitted only for embeddings and is exempt.

- Stage 1: zero model runs. Reasoning spend $0. Embeddings-exempt spend $0 (none used).
- Stage 2 deterministic: zero model runs. $0.
- Stage 2 reasoning: 14 cases × 2 arms × 3 draws = 84 case-runs × $0.24 = $20.16. Targeted repeat: 5 cases × 2 arms × 5 draws = 50 × $0.24 = $12.00. Preflight total $32.16.
- Embeddings-exempt spend: $0 planned (mock server or none; corpus-scale OpenAI embedding would be ~$0.19 per 9.6M tokens if ever needed — listed separately, not planned).
- Hard ceiling: $40. Stop all runs at the ceiling regardless of state.

## Abort criteria

- Any reasoning run bills an API key (fail-closed breach): abort immediately, file defect.
- Stage 2 flag-on deterministic gate below 30/30: abort the reasoning pass entirely (feature defective; do not burn draws).
- Search p95 with flag on exceeds 2× the flag-off measurement mid-run: abort, investigate.
- Verbatim payload exceeds the 9,600-char cap or query-count assertion fails: abort.
- Spend reaches $40.

## Reporting

The run record must contain: git commit fingerprint with clean-tree confirmation; flag state per run; per-scale n/30 tables for both flag states; per-draw claim scores and the paired aggregate with McNemar p and CIs; payload char deltas and verbatim payload sizes; search p95 table vs baseline; query counts; cost actuals vs preflight, subscription vs embeddings-exempt listed separately; full artifact paths under results/.

# E02 — Verbatim Identifier Gate

Status: Specified — not run
Date: 2026-07-27
Gates: D02 (D02-verbatim-span-contract.md)
Phase: 0 (stage 1, harness-only, pre-code) then 1 (stage 2, requires D02 built behind verbatim_spans)

## Question

Does the search payload return literal identifier lines verbatim when the matching line lies beyond the 2,400-char excerpt window — and does D02's verbatim_matches close the documented paraphrase/exact-value loss without payload bloat or reasoning regression?

Stage 1 exists to document the defect on the current build before any code changes. Stage 2 gates the fix. Both stages become permanent harness gates.

## Preconditions and build items

- B1 (S): Extend performance_eval.py synthetic_documents generation to plant a unique identifier token (format `STRAYID-<scale>-<n>-<hex8>`) at a byte offset strictly greater than 2,400 into 30 selected documents per scale, recording (path, identifier, offset) in the fixture manifest. Anchor: performance_eval.py synthetic corpus generator.
- B2 (S): Verbatim-return checker: for each planted identifier, issue a search and assert the exact identifier string appears verbatim in the raw search response payload (in-payload only; follow-up open/read does not count). Register as named gate `verbatim_identifier` in the performance_eval gate set. Anchor: performance_eval.py correctness-marker gates.
- B3 (M, stage 2 only): D02 built behind verbatim_spans — exact-lane response assembly in apps/api/src/simple_core.rs plus byte-for-byte passthrough in apps/mcp memory.query.
- B4 (S): n≥3 paired-draw aggregator eval/aggregate_draws.py (per-case win/loss/tie, exact-binomial McNemar, case-level bootstrap CIs, stdlib only) — known build item, does not exist yet; shared, specified in E01-paired-draw-machinery-and-baseline.md.
- B5 (S): Identifier-heavy case tag in eval/recent_work_cases.json covering D02's full chronic identifier list (D02-verbatim-span-contract.md): recent-aether-gmail-actions, recent-europe-calendar-dedup, recent-aether-morning-brief, and the tracker cases, for targeted repeats.

Stage 1 requires only B1+B2. Embeddings are unnecessary: the probe is exact-lane, and all latency baselines are exact+lexical; use tests/mock_openai_embeddings.py only if a run insists on semantic coverage.

## Arms

- Stage 1 (Phase 0): single arm — current build, no flag. Expected result: FAIL (0/30 identifiers verbatim in-payload per scale). The failing artifact is the defect record cited by D02.
- Stage 2 deterministic: verbatim_spans off vs on; 30 planted identifiers per scale.
- Stage 2 reasoning: service_api with flag off vs service_api with flag on, recent-work manifest, n≥3 paired draws; optional targeted 5-draw repeat on the identifier-heavy tag if the headline delta sits inside the ±3-5 claim noise floor.

## Corpus and fixtures

- Synthetic performance corpus at default scales 1k/10k/64k plus 640k via --future-soak; 30 planted identifiers per scale, all past char 2,400, mixed positions (mid-document and tail) and mixed section depths.
- Reasoning: the recent-work fixture corpus used by agent_work_eval (12 cases / 48 claims), model gpt-5.6-sol from manifest.

## Procedure

1. Land B1+B2 on a clean git tree (implementation fingerprint gate requires it).
2. Stage 1 run: `python performance_eval.py run --label verbatim-identifier-stage1 --out results/2026-MM-DD-verbatim-identifier-stage1.json` at default scales, 30 samples (definitive; `--quick` acceptable only for smoke).
3. Record the expected failure per scale (target: 0/30 verbatim in-payload). If any identifier does return verbatim, record which lane and position, and re-scope D02 before proceeding.
4. Mark `verbatim_identifier` as a known-failing documented gate until D02 ships. Stage 1 is complete; no model runs occurred.
5. Stage 2 begins only after B3 lands on a clean tree. Deterministic pass: `python performance_eval.py run --label verbatim-identifier-stage2-off --out results/2026-MM-DD-verbatim-identifier-stage2-off.json` with flag off (failure must persist unchanged), the same with `--label verbatim-identifier-stage2-on` and flag on at 1k/10k/64k, then `python performance_eval.py run --label verbatim-identifier-stage2-soak --future-soak --out results/2026-MM-DD-verbatim-identifier-stage2-soak.json` with flag on. Require 30/30 at every scale with flag on.
6. Reasoning pass, paired draws. For N in 1..3, flag off: `python agent_work_eval.py run --manifest eval/recent_work_cases.json --condition service_api --concurrency 3 --timeout 360 --run-id verbatim-off-draw<N> --out results/2026-MM-DD-verbatim-off-draw<N>.json --report results/2026-MM-DD-verbatim-off-draw<N>.md`. Flip the runtime flag on (record flag state in the run record) and repeat with slug verbatim-on-draw<N>.
7. Aggregate with B4: `python eval/aggregate_draws.py results/2026-MM-DD-verbatim-*-draw*.json --out results/2026-MM-DD-verbatim-aggregate.json` — per-case pairing across draws, exact McNemar, bootstrap CIs.
8. If overall delta is inside the noise floor but identifier-tagged cases move, run the 5-draw targeted repeat on the tagged subset (both arms) and aggregate separately.
9. Use `agent_work_eval.py regrade` for any rubric corrections; never regenerate answers to fix grading.
10. Promote `verbatim_identifier` to a permanent, blocking performance_eval gate: with flag on it fails closed below 30/30; with flag off it remains a documented expected-fail.

## Metrics

- Verbatim in-payload rate, n/30 per scale, per flag state.
- Search p95 flag on vs off at each scale, vs the 53.1ms v8 640K baseline and the ≤3,000ms hard gate.
- Payload chars/case delta (overfetch guard; reference legacy ~41,441 vs RuptureOps ~70,814 service chars/case) and verbatim payload chars vs the 9,600-char cap.
- Claims per draw (n/48), per-case win/loss/tie, exact McNemar p; identifier-tagged case wins.
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
- Stage 2 reasoning: 12 cases × 2 arms × 3 draws = 72 case-runs × $0.24 = $17.28. Targeted repeat worst case with the full B5 tag list: 6 tagged cases × 2 arms × 5 draws = 60 × $0.24 = $14.40. Preflight total $31.68.
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

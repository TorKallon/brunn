# E01 — Paired-Draw Machinery and Baseline Parity

Status: Specified — not run
Date: 2026-07-27
Gates: D01 (overfetch/context-budget design — its priority is contingent on this experiment's overfetch replication); supplies the mandatory n≥3 machinery every subsequent Dxx acceptance gate cites
Phase: 0 (harness-only, pre-code)

## Question

At n≥3 paired draws, is the simplified core's `service_api` condition non-inferior to direct filesystem access — both read-only and with a writable sidecar — across the full 5-suite matrix? And does the overfetch diagnosis (~70,814 service chars/case vs legacy 41,441 on RuptureOps) replicate, or was it an n=1 artifact?

Single-draw suite scores swing ±3-5 claims (agent-work native 40→47→44→43→47 across builds). Every parity claim to date — 170/228 vs 160/228 vs 171/228, native API 186/228 vs files 194/228 — is inside or near that noise floor. This experiment builds the machinery that makes any of those comparisons load-bearing, then produces the first statistically grounded baseline.

## Preconditions and build items

1. **Draw aggregator** (Small; new file eval/aggregate_draws.py, stdlib only). Repeat driver already exists via distinct `--run-id` per draw. The aggregator consumes per-draw result JSONs and emits: per-case win/loss/tie across paired conditions, exact-binomial McNemar on discordant pairs (`math.comb`, no scipy), and case-level bootstrap CIs (resample cases with replacement, ≥10,000 iterations, `random` module) on claim-score differences. Also aggregates per-case service/context character counts per condition.
2. **Writable-sidecar filesystem condition** (Small in agent_work_eval.py; Medium in transition_eval.py). New condition `filesystem_sidecar`: a writable `./sidecar` directory beside the read-only corpus symlink, prompt extended to require writing a checkpoint file, accounting recorded in `persisted_checkpoints`. The current filesystem condition is instruction-restricted read-only — a handicapped control. transition_eval.py seeds currently accept only workspace checkpoints; the sidecar seed extension is the Medium item. OWNER DECISION: if the Medium item would delay the launch of draws, run transitions with two arms (`service_api_resume` vs `filesystem_rebuild`) in this experiment and add the sidecar transitions arm in E10 (E10-combined-preflight.md).
3. **Overfetch instrumentation check** (Small). Verify per-case served-character accounting is captured in run outputs for all three conditions so the aggregator can re-derive chars/case; the ~70.8K figure rests on one run and D01's priorities hang on it.

No product code changes. `service_api` targets the existing simplified core on Nyx.

## Arms

- A: `service_api` (simplified core, current flags/defaults, semantic lane in its current pending state — matching all cited latency evidence).
- B: `filesystem` (read-only, instruction-restricted).
- C: `filesystem_sidecar` (writable sidecar, checkpoint required).

All arms use the manifest model (gpt-5.6-sol) via the Codex subscription.

## Corpus and fixtures

Full 5-suite matrix, 57 cases / 228 claims: agent-work 13/52, recent 12/48, rupture 12/48, personal 15/60, transitions 5/20. Same fixtures across all draws and arms; implementation fingerprint requires a clean git tree recorded per draw.

## Procedure

MM-DD below is the actual run date.

1. Validate: `python agent_work_eval.py validate --manifest eval/work_cases.json` (repeat for eval/recent_work_cases.json, eval/rupture_ops_cases.json, eval/personal_coordination_cases.json); `python transition_eval.py validate`. (Both harnesses live at repo root; eval/ holds only manifests and fixtures.)
2. For each draw N in 1..3, for each manifest M in {work, recent_work, rupture_ops, personal_coordination}:
   `python agent_work_eval.py run --manifest eval/M_cases.json --condition service_api --condition filesystem --condition filesystem_sidecar --concurrency 3 --timeout 360 --run-id e01-M-draw<N> --out results/2026-MM-DD-e01-M-draw<N>.json --report results/2026-MM-DD-e01-M-draw<N>.md`
3. For each draw N: `python transition_eval.py run --condition service_api_resume --condition filesystem_rebuild --embeddings none --run-id e01-transitions-draw<N> --out results/2026-MM-DD-e01-transitions-draw<N>.json --report results/2026-MM-DD-e01-transitions-draw<N>.md` (add a third `--condition` for the sidecar arm if build item 2's Medium half landed).
4. Scoring iteration uses regrade only — `python agent_work_eval.py regrade --input results/... --out results/...` rescores saved answers without regeneration; never re-run generation to fix a rubric.
5. Aggregate: `python eval/aggregate_draws.py results/2026-MM-DD-e01-*-draw*.json --out results/2026-MM-DD-e01-aggregate.json`.

## Metrics

- Per-suite and corpus-wide paired win/loss/tie; McNemar exact p per pairing (A-B, A-C, B-C); bootstrap 95% CI on mean per-case claim difference.
- Chars/case per condition per suite, with CI; overfetch replication tested on the rupture suite specifically, with the legacy 41,441 chars/case figure cited as a fixed external reference.
- Two-sided checkpoint comparison: `persisted_checkpoints` rate and content in arm C sidecars vs service checkpoints in arm A (deterministic id, 11 rows/~55KB) — both directions: does the service make durable continuation more reliable than free-form files, and do sidecar files carry anything the service format drops.
- Chronic-case tracking (reported, not gated): ruptureops-archive-import-reconciliation, ruptureops-flowworks-campaign-revision, ruptureops-spatial-evidence, ruptureops-forked-agent-idempotency, recent-europe-calendar-dedup, recent-aether-gmail-actions, recent-aether-morning-brief, coord-deadline-readiness; transitions claim-slot omissions (straylight-api-gate-transition worst).

## Acceptance criteria

1. Machinery: aggregator produces all three pairings from real draw data; a synthetic self-vs-self run yields McNemar p ≈ 1 and CI covering 0 (sanity check).
2. Parity statement (A vs C, the fair control): non-inferiority declared iff McNemar two-sided p ≥ 0.05 against A AND the bootstrap 95% CI lower bound on corpus-wide claim difference is above −5 claims (the documented single-draw noise floor). Superiority on any suite requires McNemar p < 0.05 in A's favor.
3. Overfetch verdict: replicated iff mean rupture-suite service chars/case exceeds the filesystem arm's by >15,000 chars with a 95% CI excluding 0 across the 3 draws. If it does not replicate, D01's priority is formally downgraded and the aggregate must say so.
4. All numbers reported with draw count and CI; no single-draw deltas quoted without the noise-floor caveat.

## Cost preflight and ceiling

Reasoning runs: 52 agent-suite cases × 3 conditions × 3 draws = 468, plus transitions 5 cases × 3 draws × 3 conditions (45) if the sidecar transitions arm lands — 513 runs worst case — or × 2 conditions (30) on the default OWNER-DECISION deferral path, 498 runs. At the observed all-in $0.24/agent-run equivalent (470-run audit, $113.18): 513 × $0.24 ≈ $123.12 worst case; 498 × $0.24 ≈ $119.52 on the default path. All reasoning goes through the ChatGPT-authenticated Codex subscription, fail-closed (`require_codex_subscription` rejects API keys); the $0.24 is the audited subscription-equivalent rate, not usage billing. Regrades regenerate nothing and are treated as marginal. Embeddings-exempt spend: $0 — no embedding backfill in this experiment; the semantic lane stays in its current pending state.

Ceiling: **$150** hard. OWNER DECISION: explicit owner approval required before launching the draws (2026-07-27 cost-audit rule); this spec is not that approval.

## Abort criteria

- Cumulative cost tracker crosses $150: stop immediately, aggregate whatever complete draws exist.
- >10% of runs in any draw fail on harness/infra errors: stop, fix, restart that draw with a fresh run-id (do not mix partial draws).
- Nyx simplified-core instability (any 5xx burst or checkpoint-lineage anomaly): stop; a lineage anomaly is a defect report, not an eval artifact.
- Grader instability: if regrade of an unchanged rubric shifts any suite by >2 claims, freeze and debug grading before continuing.

## Reporting

The run record (results/2026-MM-DD-e01-aggregate.json + companion Markdown) must contain: git SHA and clean-tree fingerprint per draw; all run-ids and per-draw artifact paths; per-suite paired tables with McNemar p and CIs; chars/case per condition with the overfetch verdict stated in one sentence; the two-sided checkpoint comparison; chronic-case per-draw outcomes; cost actuals vs the $123.12 preflight; and the parity statement in the exact non-inferiority language of acceptance criterion 2. This record becomes the baseline every later experiment (E10-combined-preflight.md) diffs against.

## References

- Noise floor and parity history: 57-case strict draw (170/160/171 of 228); interface run 186 vs 194/228; agent-work variance 40→47→44→43→47 — vault eval notes, 2026-07-2x.
- Overfetch n=1: RuptureOps ~70,814 vs legacy 41,441 chars/case — vault overfetch diagnosis note.
- Cost basis: 470-run audit ($113.18, ≈$0.24/run); Decisions.md cost rules.
- D01 (context/overfetch budget design); D09-latency-contract-and-gates.md (deterministic counterpart); E10-combined-preflight.md (consumer of this machinery).

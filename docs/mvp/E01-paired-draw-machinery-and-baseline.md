# E01 — Paired-Draw Machinery and Baseline Parity

Status: Complete — non-inferiority not established; paired overfetch absent
Date: 2026-07-28
Gates: D01 (overfetch/context-budget design — its priority is contingent on this experiment's overfetch replication); supplies the mandatory n≥3 machinery every subsequent Dxx acceptance gate cites
Phase: 0 (harness-only, pre-code)

## Result

The definitive 2026-07-28 run completed all 531 untouched case-runs at three
paired draws. The full record is
[results/2026-07-28-e01-aggregate.md](../../results/2026-07-28-e01-aggregate.md);
the machine aggregate is
[results/2026-07-28-e01-aggregate.json](../../results/2026-07-28-e01-aggregate.json),
and the 177-session operation audit is
[results/2026-07-28-e01-operation-accounting-audit.json](../../results/2026-07-28-e01-operation-accounting-audit.json).

- `service_api` minus `filesystem_sidecar` was -4.667 claims per
  236-claim corpus draw, with case-clustered bootstrap 95% CI
  [-13.667, 4.333]. The lower bound is not above the -5-claim margin, so
  non-inferiority is not declared. This does not prove inferiority.
- RuptureOps `service_api` produced 63,090 model-visible tool-output
  characters per case versus 142,640 for `filesystem`; the paired difference
  was -79,549 with 95% CI [-111,508, -49,126]. Paired service-versus-files
  overfetch is absent under the predeclared rule.
- All 174 checkpoint-eligible service sessions persisted; one committed a
  duplicate successful checkpoint. Three read-only controls were correctly
  ineligible. Sidecars persisted in 176/177 sessions. Service transition
  lineage was exact in 15/15 cards; sidecars preserved exact parent linkage in
  13/15.
- A hash-bound event audit found 20 local checkpoint command failures across
  17 sessions: 14 unsupported or misnamed flag argparse rejections, one
  unsupported `--json-stdin` rejection, one shell-quoting rejection, and four
  wrapper-local status-0 invalid payload rows. Every affected writable session
  later persisted. The service itself recorded 520 HTTP-200 operations, zero
  denials, zero 4xx, and zero 5xx.
- No behavior-selected repair or replacement run was used. All local failure
  output remains in `model_visible_tool_output_chars`; only service-call
  accounting is normalized to actual HTTP operations.
- The run used 534 case-runs including the excluded three-arm calibration,
  equivalent to $128.16 at the predeclared subscription-equivalent rate. Actual
  API and embedding spend was $0.

The local retry pattern is a harness prompt defect: E01 said only
`./memory checkpoint`, so agents guessed incompatible syntax. Later harnesses
must provide one canonical checkpoint command before E04. That follow-up does
not alter or regrade E01.

## Question

At n≥3 paired draws, is the simplified core's `service_api` condition non-inferior to direct filesystem access — both read-only and with a writable sidecar — across the full 5-suite matrix? In the new paired arms, how much model-visible tool output does the service produce relative to files?

The cited ~70,814 versus 41,441 RuptureOps comparison was simplified service versus legacy service, not service versus files. E01 has no legacy-service arm and therefore cannot claim to replicate that historical comparator. The 41,441 figure remains a fixed, non-paired reference; E01's load-bearing overfetch result is the newly paired service-versus-filesystem difference.

Single-draw suite scores swing ±3-5 claims (agent-work native 40→47→44→43→47 across builds). Every parity claim to date — 170/228 vs 160/228 vs 171/228, native API 186/228 vs files 194/228 — is inside or near that noise floor. This experiment builds the machinery that makes any of those comparisons load-bearing, then produces the first statistically grounded baseline.

## Preconditions and build items

1. **Draw aggregator** (Small; new file eval/aggregate_draws.py, stdlib only). Repeat driver already exists via distinct `--run-id` per draw. The aggregator consumes complete per-draw result JSONs, rejects mixed/dirty fingerprints or missing ChatGPT-auth proof, averages repeated draws within each case, and only then resamples cases with replacement (≥10,000 iterations). It emits per-case claim win/loss/tie, exact-binomial McNemar on majority-collapsed binary case outcomes (`math.comb`, no scipy), and a clustered corpus-total claim-difference bootstrap. It also aggregates the comparable `model_visible_tool_output_chars` metric for every condition.
2. **Writable-sidecar filesystem condition** (Small in agent_work_eval.py; Medium in transition_eval.py). New condition `filesystem_sidecar`: a writable `./sidecar` directory beside the read-only corpus symlink, prompt extended to require writing a checkpoint file, and content/hash/size/validity accounting recorded in `persisted_checkpoints`. Transition sidecars must preserve the exact parent checkpoint, N+1 revision, prior source, delta source, and a complete child state.
3. **Overfetch instrumentation check** (Small). Every record carries `model_visible_tool_output_chars`, defined identically as characters returned by completed Codex command executions. Service-specific source/metadata/replay metrics remain diagnostic and must not be substituted for the cross-arm metric.
4. **Immutable run ledger** (Small). Every draw records git SHA and clean-tree state plus Codex executable path/version and the timestamped `Logged in using ChatGPT` authentication check. Aggregation fails closed on missing or mixed provenance.

No product code changes. `service_api` targets the existing simplified core on Nyx.

## Arms

- A: `service_api` (simplified core, current flags/defaults, semantic lane in its current pending state — matching all cited latency evidence).
- B: `filesystem` (read-only, instruction-restricted).
- C: `filesystem_sidecar` (writable sidecar, checkpoint required).

All arms use the manifest model (gpt-5.6-sol) via the Codex subscription.

## Corpus and fixtures

Full 5-suite matrix at the checked-in active manifests, 59 cases / 236
claims: agent-work 13/52, recent 14/56, rupture 12/48, personal 15/60,
transitions 5/20. Same fixtures across all draws and arms; implementation
fingerprint requires a clean git tree recorded per draw.

## Procedure

MM-DD below is the actual run date.

1. Use the isolated Nyx preamble in
   [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md), then
   validate all five real manifests. `--manifest` is global and precedes the
   subcommand:
   `python3 agent_work_eval.py --manifest eval/work_cases.json validate`
   (repeat for `recent_work_cases.json`, `rupture_ops_cases.json`, and
   `personal_coordination_cases.json`), then
   `python3 transition_eval.py --manifest eval/transition_cases.json validate`.
2. Define the service runtime contract once:
   `E01_RUNTIME=(--expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag search_fair_share=off --expect-feature-flag search_top1_hydration=off --expect-feature-flag search_char_cap=off --expect-runtime-config search_section_demotion_top_n=null --expect-feature-flag verbatim_spans=off --expect-feature-flag resume_deltas=off --expect-feature-flag supersession_demotion=off --expect-feature-flag intention_ledger=off --expect-feature-flag read_path_roundtrip_v1=off --expect-feature-flag lexical_single_scan=off)`.
3. For each draw `N` in `1 2 3`, invoke each real manifest explicitly:
   `python3 agent_work_eval.py --manifest eval/work_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --condition filesystem --condition filesystem_sidecar "${E01_RUNTIME[@]}" --concurrency 3 --timeout 360 --run-id "e01-work-draw${N}" --out "results/2026-MM-DD-e01-work-draw${N}.json" --report "results/2026-MM-DD-e01-work-draw${N}.md"`.
   Repeat with manifest/slug pairs `recent_work_cases.json`/`recent`,
   `rupture_ops_cases.json`/`rupture`, and
   `personal_coordination_cases.json`/`personal`.
4. For each draw:
   `python3 transition_eval.py --manifest eval/transition_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api_resume --condition filesystem_rebuild --condition filesystem_sidecar "${E01_RUNTIME[@]}" --embeddings none --run-id "e01-transitions-draw${N}" --out "results/2026-MM-DD-e01-transitions-draw${N}.json" --report "results/2026-MM-DD-e01-transitions-draw${N}.md"`.
5. Scoring iteration uses regrade only. For example:
   `python3 agent_work_eval.py --manifest eval/work_cases.json regrade --input results/2026-MM-DD-e01-work-draw1.json --out results/2026-MM-DD-e01-work-draw1-regraded.json`.
   Never regenerate to fix a rubric, and do not mix original and regraded
   versions of the same draw in aggregation.
6. Build an explicit draw-only input array and aggregate:
   `E01_ARTIFACTS=(results/2026-MM-DD-e01-{work,recent,rupture,personal,transitions}-draw{1,2,3}.json); python3 eval/aggregate_draws.py "${E01_ARTIFACTS[@]}" --expected-arm-retrieval-modes service_api=exact,lexical --expected-arm-retrieval-modes service_api_resume=exact,lexical --out results/2026-MM-DD-e01-aggregate.json`.

## Metrics

- Per-suite and corpus-wide paired claim win/loss/tie; exact McNemar p per pairing (A-B, A-C, B-C) on majority-collapsed binary case outcomes; bootstrap 95% CI on the corpus-total claim difference after repeated draws are averaged within case.
- Comparable model-visible tool-output chars/case per condition per suite, with a case-clustered CI on the paired difference. The RuptureOps service-versus-files result is the experiment; legacy 41,441 chars/case is descriptive historical context only.
- Two-sided checkpoint comparison: `persisted_checkpoints` rate and content in arm C sidecars vs service checkpoints in arm A (deterministic id, 11 rows/~55KB) — both directions: does the service make durable continuation more reliable than free-form files, and do sidecar files carry anything the service format drops.
- Chronic-case tracking (reported, not gated): ruptureops-archive-import-reconciliation, ruptureops-flowworks-campaign-revision, ruptureops-spatial-evidence, ruptureops-forked-agent-idempotency, recent-europe-calendar-dedup, recent-aether-gmail-actions, recent-aether-morning-brief, coord-deadline-readiness; transitions claim-slot omissions (straylight-api-gate-transition worst).

## Acceptance criteria

1. Machinery: aggregator produces all three pairings from complete real draw data; a synthetic self-vs-self run yields exact McNemar p = 1 and a clustered CI equal to or covering 0.
2. Parity statement (A vs C, the fair control): non-inferiority is declared iff the case-clustered bootstrap 95% lower bound on the expected full-corpus total claim difference (A−C, after averaging draws within case) is above −5 claims. Report McNemar separately as a test of asymmetric binary case outcomes; p≥0.05 is not evidence of equivalence or non-inferiority. Superiority on a suite requires McNemar p<0.05 in A's favor and a claim-difference CI above 0.
3. Overfetch verdict: paired service overfetch is present iff mean RuptureOps `model_visible_tool_output_chars` exceeds the filesystem arm by >15,000 chars and the clustered 95% CI excludes 0. State explicitly that this does or does not establish new service-versus-files overfetch; never call it a replication of the old simplified-versus-legacy number.
4. All numbers reported with draw count and CI; no single-draw deltas quoted without the noise-floor caveat.

## Cost preflight and ceiling

Reasoning runs at the checked-in active manifest counts: 54 agent-suite cases
(work 13, recent 14, rupture 12, personal 15) × 3 conditions × 3 draws =
486, plus transitions 5 × 3 × 3 = 45, for **531 case-runs**. At the observed
all-in $0.24/agent-run equivalent (470-run audit, $113.18), 531 × $0.24 =
**$127.44**. All reasoning goes through the ChatGPT-authenticated Codex
subscription, fail-closed (`require_codex_subscription` rejects API keys); the
$0.24 is an audited subscription-equivalent rate, not API usage billing.
Regrades regenerate nothing and are treated as marginal. Embeddings-exempt
spend: $0 — run the isolated E01 stack without a real embedding worker so
semantic state cannot change mid-draw.

Ceiling: **$150** hard. OWNER DECISION: explicit owner approval required before launching the draws (2026-07-27 cost-audit rule); this spec is not that approval.

Execution remained below the ceiling: 531 definitive case-runs plus three
excluded calibration case-runs yielded a $128.16 subscription-equivalent.
There was no paid embedding or other OpenAI API spend.

## Abort criteria

- Cumulative cost tracker crosses $150: stop immediately, aggregate whatever complete draws exist.
- >10% of runs in any draw fail on harness/infra errors: stop, fix, restart that draw with a fresh run-id (do not mix partial draws).
- Nyx simplified-core instability (any 5xx burst or checkpoint-lineage anomaly): stop; a lineage anomaly is a defect report, not an eval artifact.
- Grader instability: if regrade of an unchanged rubric shifts any suite by >2 claims, freeze and debug grading before continuing.

## Reporting

The run record (results/2026-MM-DD-e01-aggregate.json + companion Markdown) must contain: immutable run ledgers with git SHA/clean state and Codex path/version/auth timestamp per draw; all run-ids and per-draw artifact paths/hashes; per-suite paired tables with McNemar p and clustered CIs; comparable chars/case per condition with the paired overfetch verdict stated in one sentence; the two-sided checkpoint comparison; chronic-case per-draw outcomes; cost actuals vs the $127.44 preflight; and the parity statement in the exact non-inferiority language of acceptance criterion 2. This record becomes the baseline every later experiment (E10-combined-preflight.md) diffs against.

## References

- Noise floor and parity history: 57-case strict draw (170/160/171 of 228); interface run 186 vs 194/228; agent-work variance 40→47→44→43→47 — vault eval notes, 2026-07-2x.
- Overfetch n=1: RuptureOps ~70,814 vs legacy 41,441 chars/case — vault overfetch diagnosis note.
- Cost basis: 470-run audit ($113.18, ≈$0.24/run); Decisions.md cost rules.
- D01 (context/overfetch budget design); D09-latency-contract-and-gates.md (deterministic counterpart); E10-combined-preflight.md (consumer of this machinery).

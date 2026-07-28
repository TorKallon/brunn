# E11 — Wiki-Link Leads paired experiment (service_api ± link_leads vs filesystem)

Status: Prerequisite abort — D02 rejected; D06 and owner manifest absent
Date: 2026-07-27
Gates: D06
Phase: 1 (requires flagged feature build)

**CONDITIONAL EXPERIMENT. Hard precondition: the owner corpus is imported to the Nyx simplified core with a passed Tier A fidelity audit. The owner vault is link-rich; synthetic fixtures are not — running this on fixture corpora is invalid and its results must be discarded, not reported.**

**CURRENT PREREQUISITE ABORT (2026-07-28):** Do not run E11. E02 rejected
D02, so D06's hard `D01 + D02 landed` dependency is false. D06 and its
`link_leads` runtime surface are not implemented, and the required 8–10-case
manifest also has not been authored in the owner's words and signed off by the
owner. Passing snapshot tooling or a Tier A fidelity audit cannot substitute
for these gates. This abort is not a result against link leads; once D02 is
accepted, D06 is eligible and implemented, and the owner-authored,
owner-signed-off manifest exists, the experimental intent below remains
unchanged.

## Question

Does pointer-only `linked_leads` (D06-wiki-link-leads.md) improve paired claim outcomes on agent-work and link-heavy owner cases without inflating context chars or regressing write latency — and do agents actually follow the leads into rubric-accepted sources?

## Preconditions and build items

1. **NOT SATISFIED.** D06 implemented behind `link_leads` flag — Medium (apps/api/src/simple_core.rs: search response assembly + worker reindex link parser; derived link table). At source revision `d989ae5893e24a14d11d51ad9c4cfc8c8b812e1b`, exact source inspection under `apps/` and `eval/` finds no `link_leads`/`linked_leads` implementation. D06 must not be built or activated while its rejected D02 prerequisite remains false.
2. Owner corpus imported to Nyx simplified core, Tier A fidelity audit passed (paths/bytes/sha256 identical; parent_checkpoint_id resolution) — Medium (imports/ tooling).
3. Arm-aware n≥3 aggregator and authenticated runtime snapshot contract — implemented; see [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
4. Lead-follow instrumentation: parse eval transcripts to match subsequent open/read calls against `linked_leads` returned earlier in the same case — Small (agent_work_eval.py transcript post-processing; no harness behavior change).
5. **NOT SATISFIED.** Link-heavy owner case manifest
   `eval/owner_link_cases.json` (8–10 cases; selection procedure below) —
   Small. The file is currently absent and untracked. It must contain questions
   authored in the owner's words and receive explicit owner sign-off before
   draw 1. Do not solicit that input while the rejected D02 and absent D06
   implementation independently prohibit the run.
6. Reindex-churn hook for the soak: run link-parse worker jobs continuously during performance_eval write probes — Small (performance_eval.py).
7. Treatment-arm activation plumbing — Small. D06 attaches leads only on request `expand_links: true` or the relational heuristic (an open OWNER DECISION), and agent_work_eval.py has no per-request flag plumbing; without this item the treatment arm returns zero leads and the experiment is vacuous. Use D06's evaluation-only runtime config `link_leads.force_attach=true` (D06-wiki-link-leads.md, Activation) for the treatment arm; verify with a pre-draw smoke search that a `linked_leads` array is actually present.

The deterministic current-snapshot inventory, scoped text-fidelity audit,
link-candidate selection, and non-echoing leak-check commands are in
[Tier-A-owner-snapshot-tooling.md](Tier-A-owner-snapshot-tooling.md). Live
inventory/candidate/leak artifacts stay under ignored `operator-output/`; no
owner content or rubric answer is committed by the scaffold. Its supported-text
pass is not a substitute for D14's full history/binary/checkpoint fidelity gate.

## Arms

1. **service_api, link_leads OFF** (control).
2. **service_api, link_leads ON** (`expand_links` activation per D06) — the paired treatment.
3. **filesystem** (instruction-restricted read-only) — reference anchor only, single draw, not part of the paired statistic. Interface-run context: native API 186/228 vs files 194/228 on the simplified core.

Paired analysis is arm 1 vs arm 2, per case, per draw, identical manifests and corpus.

## Corpus and fixtures

Owner vault on Nyx simplified core. Embeddings pending is the required profile — all existing latency evidence is exact+lexical, and lead parsing is purely lexical; do not introduce a semantic confound. Case sets:

- agent-work manifest `eval/work_cases.json` (14 cases / 56 claims).
- Owner link-heavy set, 8–10 cases, selected WITHOUT leaking rubric answers into the corpus:
  1. From the derived link table, enumerate vault notes with ≥3 outgoing resolved links.
  2. The owner writes questions whose answers require ≥2 linked notes, phrased in the owner's own words without quoting answer text from any note.
  3. Rubric claims are authored in a second pass against the source notes and stored ONLY in `eval/owner_link_cases.json` — never written into the vault.
  4. Leak gate: grep-verify no rubric claim string appears verbatim in the corpus; any hit → rewrite the claim.
  5. OWNER DECISION: sign-off on the final case list before draw 1.

## Procedure

1. Preflight: clean git tree (implementation fingerprint gate); confirm `link_leads` default off; `python3 agent_work_eval.py --manifest eval/work_cases.json validate` and `python3 agent_work_eval.py --manifest eval/owner_link_cases.json validate`.
2. For draw N in 1..3, arms interleaved within the day (control then treatment, same corpus state, no writes between):
   - Control: `python3 agent_work_eval.py --manifest eval/work_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e11-control --paired-draw-id e11-work-draw<N> --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=on --concurrency 3 --timeout 360 --run-id e11-control-work-run<N> --out results/2026-MM-DD-e11-linkleads-ctl-draw<N>.json --report results/2026-MM-DD-e11-linkleads-ctl-draw<N>.md`, with the D06 status fields expected off. Repeat against `eval/owner_link_cases.json` using `--paired-draw-id e11-owner-draw<N>`.
   - Treatment: identical commands with `--experiment-arm e11-treatment`, the same suite-specific paired-draw IDs, and D06's surface/force-attach status fields expected on. Before draw 1, run the build-item-7 smoke search and confirm `linked_leads` is present.
   These examples bind only the currently recognized non-confounds
   (`semantic_lane=off`, `verbatim_spans=on`, and exact+lexical retrieval).
   They remain ineligible until D06 exposes authenticated runtime status keys
   for link-lead surface and evaluation-only force-attach state. Once those
   keys exist, the control command must assert both off and the treatment
   command must assert both on; an artifact without those assertions is not an
   E11 result.
3. Filesystem anchor, once: both manifests with `--condition filesystem`, run-id `e11-fs-draw1`, artifacts `results/2026-MM-DD-e11-linkleads-fs-draw1.json`.
4. Reindex soak: `python performance_eval.py run --label e11-reindex-soak --future-soak --out results/2026-MM-DD-e11-reindex-soak.json`, 30 samples, with link-parse churn enabled; capture write p95, unrelated-write p95, concurrent write/search probe, GIN idx_scan deltas.
5. Lead-follow extraction over treatment transcripts (build item 4); emit per-case lead table.
6. Aggregate only the control/treatment artifacts, excluding the one-draw filesystem anchor: `python3 eval/aggregate_draws.py <control-and-treatment-jsons> --expected-arm e11-treatment --expected-arm e11-control --expected-arm-retrieval-modes e11-treatment=exact,lexical --expected-arm-retrieval-modes e11-control=exact,lexical --out results/2026-MM-DD-e11-aggregate.json`. Use `regrade` only for scoring fixes.

## Metrics

- Paired claims per case, exact-binomial McNemar across the 3 draws (the only load-bearing quality number; single-draw swings are ±3–5 claims noise).
- Service chars/case: treatment must stay within ±2% of control.
- Turns/case (diagnostic for lead-following behavior).
- **Lead-follow yield**: fraction of followed leads landing in rubric-accepted sources; ≥20% required else kill. Also report follow rate (leads followed / leads returned) as a diagnostic.
- Write p95 and unrelated-write p95 during reindex soak vs v8 baseline (29.0ms concurrent write, results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json); protocol-to-evidence ratio ≤1.0; GIN idx_scan deltas.

## Acceptance criteria

Pass (feeds D06 acceptance gates) requires ALL:

1. No significant paired claims loss (McNemar, α=0.05); non-inferiority is the floor, a paired win is the target.
2. Lead-follow yield ≥20%.
3. Chars/case within ±2% of control on both suites.
4. Soak: all hard latency gates met AND no unexplained drift vs v8 write baselines (any >2x movement on unrelated-write p95 is a defect per the v5 3,404ms / v7 3,170ms precedent, even inside gates).
5. Protocol-to-evidence ratio ≤1.0 maintained.

Any single failure → D06 kill criteria apply.

## Cost preflight and ceiling

Arithmetic (all-in equivalent at the audited $0.24/agent-run, 470-run audit, $113.18):

- Cases per condition per draw: 14 agent-work + 10 owner = 24.
- Paired arms: 2 arms × 3 draws × 24 = 144 runs.
- Filesystem anchor: 1 × 24 = 24 runs.
- Total 168 runs × $0.24 = **$40.32**, leaving $9.68 (~40 case-runs) for timeout retries.

Subscription rule: all reasoning runs go through the ChatGPT-authenticated Codex subscription, fail-closed (require_codex_subscription rejects API keys); $0.24 is the audited all-in equivalent, not marginal API spend. Embeddings-exempt spend, listed separately: $0 required — E11 runs embeddings-pending and lead parsing is lexical; if the owner opts to index the corpus anyway, budget ≈$0.19 (usage-billed OpenAI, exempt) per 9.6M-token corpus. Soak runs are local and free.

**Hard ceiling: $50 all-in equivalent.**

## Abort criteria

- Any checkpoint-lineage incident during runs → immediate abort (matches the Tier C shadow tripwire).
- Reindex soak breaches any hard latency gate → abort treatment soak, file defect (v5/v7 shape), do not continue draws on a regressed build.
- Treatment chars/case >+5% vs control in 2 consecutive draws → abort (overfetch shape).
- Cost ledger crossing $50 equivalent, or ANY usage-billed reasoning spend detected (fail-closed violation) → abort immediately.
- Harness timeouts >20% of cases in any draw → abort, debug harness, restart the draw; do not burn paired draws on harness noise.

## Reporting

The run record must contain: git commit fingerprint (clean tree) and flag states per run; manifest hashes; all per-draw artifact paths (`results/2026-MM-DD-e11-linkleads-{ctl,trt}[-owner]-draw<N>.json` + `.md`, `...-fs-draw1.json`); aggregator output (per-case win/loss/tie, McNemar p, bootstrap CIs, combined and per-suite); the lead table (returned/followed/accepted-source hits per case) and computed yield; chars/case and turns/case tables; the soak JSON with write p95s against results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json; the cost ledger; and an explicit ship/kill recommendation mapped line-by-line to the D06-wiki-link-leads.md acceptance gates.

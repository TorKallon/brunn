# E07 — Supersession Demotion and Adoption Experiment

Status: Complete — mechanism gate passed; adoption gate failed
Date: 2026-07-28
Gates: D04 (D04-supersession-current-truth.md)
Phase: 1 (requires flagged feature build)

## Result

The definitive 2026-07-28 run completed the frozen 162-session matrix at
revision `791fac70f7345e51d732e3965f4e770e9107d0e3`. The human-readable record is
[results/2026-07-28-e07-aggregate.md](../../results/2026-07-28-e07-aggregate.md);
the machine aggregate and conclusion audit are
[results/2026-07-28-e07-aggregate.json](../../results/2026-07-28-e07-aggregate.json)
and
[results/2026-07-28-e07-conclusion-audit.json](../../results/2026-07-28-e07-conclusion-audit.json).
The hash-bound adoption and execution companions are
[results/2026-07-28-e07-adoption-semantic-audit.json](../../results/2026-07-28-e07-adoption-semantic-audit.json)
and
[results/2026-07-28-e07-session-health-audit.json](../../results/2026-07-28-e07-session-health-audit.json).

- **Mechanism gate: passed.** Within the four-case supersession family, the
  flag arm won 6 paired case-instances, baseline won 2, and 4 tied. Net +4
  clears the predeclared +3 gate. The arms passed 29/48 versus 25/48 family
  claim slots. The case-clustered expected per-draw claim difference was
  +1.333 with 95% CI [-2.333, 3.667]. Majority-collapsed full-case outcomes
  had no discordant cases and exact McNemar p = 1.0. The gate passes by its
  frozen net-win rule; the wide interval and null collapsed test mean this is
  not evidence of a statistically precise effect.
- **Safety and context gates: passed.** The flag introduced zero new
  outside-family forbidden assertions. The outside-family exact McNemar
  p-value was 1.0, with no significantly negative asymmetry. Mean
  model-visible tool output was 5,685.714 characters/case for flag versus
  5,639.881 for baseline, a 1.00812659 ratio under the 1.05 ceiling.
- **Deterministic gate: passed.** The flag-off and flag-on 64K/30 foreground
  write p95 values were 21.096 ms and 27.401 ms. Flag-on remained below the
  58.0 ms hard limit and reproduced the independently reviewed zero-headroom
  query-count contract. The measured +6.305 ms difference is reported as
  observed; no numeric “within noise” threshold was predeclared, so this run
  makes no latency-parity inference beyond the hard-gate result.
- **Adoption gate: failed.** Agents emitted syntactically valid
  `supersedes` frontmatter in 7/18 eligible unprompted sessions (38.889%),
  below 50%. The shared intention cohort separately passed at 13/18 (72.222%);
  it does not repair D04's failure. A hash-bound semantic audit further found
  6/18 resolvable distinct edges, 5/18 selected projected edges, and only 3/18
  noncompeting selected emissions. One accepted self-edge was projection
  inert, and all three calendar emissions competed with an existing
  successor.

Therefore the implemented D04 retrieval mechanism passes E07, but D04 is not
ready for Tier C. The predeclared consequence applies: add assisted authoring
and qualify its adoption before relying on supersession in deployment. Keep
the frozen official scorer result unchanged; the semantic companion is a
diagnostic that shows the syntactic metric is optimistic.

All reasoning used ChatGPT-authenticated Codex with API fallback forbidden.
The 162 definitive sessions equal $38.88 at the predeclared
subscription-equivalent rate; actual usage-billed reasoning and embeddings
were both $0.

## Question

Does explicit `supersedes` frontmatter with demotion-plus-annotation (D04-supersession-current-truth.md) win the dedup/current-over-history family without harming anything else — and, separately, will agents actually author the frontmatter unprompted? The second question is load-bearing: an oracle-seeded corpus proves the mechanism, not the deployed behavior.

## Preconditions and build items

1. D04 implemented behind `supersession_demotion` — **M**. Anchors: apps/api/src/simple_core.rs scoring (the derived_penalty application site), write/import frontmatter parse path, read view parameter.
2. recent-work-v0.3 manifest: correction notes carry real `supersedes` frontmatter and `feature_families.supersession_dedup` freezes the four relevant cases — implemented.
3. Harness flag plumbing: per-arm runtime-config toggle of `supersession_demotion` from agent_work_eval.py run setup (env var or config endpoint per D04 rollout mechanism) — **S**.
4. Arm-aware n≥3 paired-draw aggregator and runtime snapshot contract — implemented; see [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
5. Adoption-arm harness: scripted unprompted agent-work write sessions where the authoring contract is documented in workspace docs but never stated in the task prompt; an eligibility oracle labels which sessions *should* have emitted `supersedes` (session writes a note that factually corrects an existing one) — **M**.

**Resolved nuisance posture (2026-07-28):** E02 rejected D02, so every E07
service and performance stack must start with
`STRAYLIGHT_VERBATIM_SPANS=false` and every measured service arm must assert
`--expect-feature-flag verbatim_spans=off`. Verbatim spans are not an E07
variable. An E07 pass cannot rehabilitate D02.

## Arms

1. **service_api-baseline** — simplified core, flag off, v0.3 corpus (frontmatter present in source text but mechanically inert).
2. **service_api-supersession** — identical, flag on.
3. **filesystem** — instruction-restricted read-only filesystem condition, same corpus. Natural control: the frontmatter is visible as raw text, so this arm tests whether the declaration alone (without demotion/annotation machinery) is enough.
4. **adoption (measurement arm, not claim-scored)** — 12 unprompted write sessions per draw against a live workspace; contract documented, never prompted; record eligible sessions and emitted frontmatter.

## Corpus and fixtures

recent-work-v0.3: the recent suite (14 cases / 56 claims) with correction notes added as real workspace fixtures — exact paths, full source text, sha256 recorded — each carrying `supersedes:` frontmatter authored in the Markdown itself (round-trip rule). No oracle hints in any task prompt. Dedup-family label list is committed in the manifest.

## Procedure

1. Preflight: use separate project-scoped stacks from
   [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md), keep
   one immutable build revision, record the clean tree, and confirm the flag
   defaults off.
2. `python3 agent_work_eval.py --manifest eval/recent_work_cases.json validate` — must pass on v0.3.
3. Make the write-latency abort rule executable before reasoning. First run a
   definitive 64K flag-off control:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --verbatim-feature-acceptance not-applicable --query-budget-profile default-safe --label e07-base-write-latency --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=off --expect-runtime-config supersession_demotion_weight=1.5 --out results/2026-MM-DD-e07-base-write-latency.json`.
   The treatment is a non-default query shape, so calibrate it rather than
   borrowing the default-safe contract:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --verbatim-feature-acceptance not-applicable --query-budget-profile calibration --label e07-supersession-write-calibration --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=on --expect-runtime-config supersession_demotion_weight=1.5 --out results/2026-MM-DD-e07-supersession-write-calibration.json`.
   That calibration intentionally exits nonzero and is not acceptance evidence.
   Review its 30-sample counts, freeze a runtime-bound contract with profile
   `e07-supersession`, and do not invent unmeasured headroom. Freeze and
   validate it:
   `E07_QUERY_BUDGET_CONTRACT="results/2026-MM-DD-e07-supersession-query-budgets.json"; test -s "$E07_QUERY_BUDGET_CONTRACT"; python3 -m json.tool "$E07_QUERY_BUDGET_CONTRACT" >/dev/null; python3 -c 'import json,sys; p=json.load(open(sys.argv[1])); assert p["schema"]=="straylight-query-budgets@v1"; assert p["profile"]=="e07-supersession"; assert p["runtime_features"]["supersession_demotion"] is True; assert p["operations"]' "$E07_QUERY_BUDGET_CONTRACT"; E07_QUERY_BUDGET_SHA256="$(shasum -a 256 "$E07_QUERY_BUDGET_CONTRACT" | awk '{print $1}')"; test -n "$E07_QUERY_BUDGET_SHA256"; chmod 0444 "$E07_QUERY_BUDGET_CONTRACT"`.
   Record the reviewer, calibration hash, contract hash, and decision, then run
   the treatment acceptance artifact:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --verbatim-feature-acceptance not-applicable --query-budget-profile e07-supersession --query-budget-contract "$E07_QUERY_BUDGET_CONTRACT" --label e07-supersession-write-latency --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=on --expect-runtime-config supersession_demotion_weight=1.5 --out results/2026-MM-DD-e07-supersession-write-latency.json`.
   Confirm the treatment artifact's `query_budget_contract.sha256` equals
   `E07_QUERY_BUDGET_SHA256`. Both definitive artifacts must pass. Its
   `scales[].concurrent_probe.write_p95_ms` must be ≤58.0ms, exactly twice the
   v8 29.0ms reference; otherwise stop before reasoning.
4. For draw N in 1..3, complete all three claim-scored arms:
   1. `python3 agent_work_eval.py --manifest eval/recent_work_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e07-base --paired-draw-id "e07-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=off --expect-feature-flag intention_ledger=off --expect-feature-flag resume_deltas=off --expect-runtime-config supersession_demotion_weight=1.5 --concurrency 3 --timeout 360 --run-id "e07-base-run${N}" --out "results/2026-MM-DD-e07-supersession-base-draw${N}.json" --report "results/2026-MM-DD-e07-supersession-base-draw${N}.md"`.
   2. Flag arm:
      `python3 agent_work_eval.py --manifest eval/recent_work_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e07-flag --paired-draw-id "e07-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=on --expect-feature-flag intention_ledger=off --expect-feature-flag resume_deltas=off --expect-runtime-config supersession_demotion_weight=1.5 --concurrency 3 --timeout 360 --run-id "e07-flag-run${N}" --out "results/2026-MM-DD-e07-supersession-flag-draw${N}.json" --report "results/2026-MM-DD-e07-supersession-flag-draw${N}.md"`.
   3. Filesystem arm, with no service runtime expectations:
      `python3 agent_work_eval.py --manifest eval/recent_work_cases.json run --condition filesystem --experiment-arm e07-filesystem --paired-draw-id "e07-draw${N}" --concurrency 3 --timeout 360 --run-id "e07-filesystem-run${N}" --out "results/2026-MM-DD-e07-supersession-filesystem-draw${N}.json" --report "results/2026-MM-DD-e07-supersession-filesystem-draw${N}.md"`.
   4. Adoption runs are pinned to the isolated flag-on stack:
      `python3 agent_work_eval.py --manifest eval/e07_e08_adoption_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e07-adoption --paired-draw-id "e07-adoption-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=on --expect-feature-flag intention_ledger=off --expect-feature-flag resume_deltas=off --expect-runtime-config supersession_demotion_weight=1.5 --concurrency 3 --timeout 360 --run-id "e07-adoption-run${N}" --out "results/2026-MM-DD-e07-adoption-raw-draw${N}.json"`;
      then
      `python3 agent_work_eval.py --manifest eval/e07_e08_adoption_cases.json measure-adoption --input "results/2026-MM-DD-e07-adoption-raw-draw${N}.json" --out "results/2026-MM-DD-e07-adoption-draw${N}.json"`.
5. After all three measurements exist, aggregate only those exact artifacts:
   `python3 agent_work_eval.py --manifest eval/e07_e08_adoption_cases.json aggregate-adoption --input results/2026-MM-DD-e07-adoption-draw1.json --input results/2026-MM-DD-e07-adoption-draw2.json --input results/2026-MM-DD-e07-adoption-draw3.json --expected-draw e07-adoption-draw1 --expected-draw e07-adoption-draw2 --expected-draw e07-adoption-draw3 --minimum-rate 0.5 --out results/2026-MM-DD-e07-adoption-aggregate.json`.
   This fails closed on duplicate/missing draws; raw-input hash drift; manifest,
   source, build, image, runtime, feature, or retrieval-mode drift; and exits
   nonzero unless both supersession and intention adoption are at least 50%.
6. Regrade disputed artifacts with the manifest before the subcommand, for
   example
   `python3 agent_work_eval.py --manifest eval/recent_work_cases.json regrade --input "$INPUT" --out "$OUTPUT"`.
7. Aggregate the three-arm main result only from draws 1-3:
   `E07_MAIN=(results/2026-MM-DD-e07-supersession-{flag,base,filesystem}-draw{1,2,3}.json); python3 eval/aggregate_draws.py "${E07_MAIN[@]}" --expected-arm e07-flag --expected-arm e07-base --expected-arm e07-filesystem --expected-arm-retrieval-modes e07-flag=exact,lexical --expected-arm-retrieval-modes e07-base=exact,lexical --require-feature-family supersession_dedup --out results/2026-MM-DD-e07-aggregate.json`.
8. Only if the three-draw flag-vs-base result is borderline, extend the two
   service arms—not filesystem or adoption—through draws 4-5 using the same
   commands, filenames, arm identities, and `e07-draw${N}` IDs. Do not add
   these partial-arm draws to `E07_MAIN`. Produce a separate five-draw
   service-only aggregate:
   `E07_SERVICE5=(results/2026-MM-DD-e07-supersession-{flag,base}-draw{1,2,3,4,5}.json); python3 eval/aggregate_draws.py "${E07_SERVICE5[@]}" --expected-arm e07-flag --expected-arm e07-base --expected-arm-retrieval-modes e07-flag=exact,lexical --expected-arm-retrieval-modes e07-base=exact,lexical --require-feature-family supersession_dedup --out results/2026-MM-DD-e07-service-five-draw-aggregate.json`.

   **Predeclared for the 2026-07-28 execution:** do not use this optional
   extension. "Borderline" has no frozen numeric definition, so inspecting the
   three-draw result before deciding would create a post-hoc stopping rule.
   Draws 1-3 and the stated acceptance criteria are definitive for this run.

## Metrics

- Claims/56 per arm per draw.
- Dedup-family paired case wins/losses/ties (flag vs baseline), per draw and summed.
- Forbidden-assertion rate: rubric-flagged assertions of a superseded (stale) fact, counted separately inside and outside the family.
- Adoption rate: eligible sessions emitting valid `supersedes` frontmatter / eligible sessions.
- Context chars/case per arm (overfetch guard; legacy reference ~41,441 chars/case).
- Deterministic 64K foreground write p95 from
  `scales[].concurrent_probe.write_p95_ms`, flag off vs on, with the treatment
  hard-bounded at 58.0ms.

## Acceptance criteria

- **Primary:** dedup-family net ≥ +3 case-instances (wins minus losses, flag vs baseline) summed across ≥3 paired draws, with McNemar exact p and bootstrap CI reported. Single-draw deltas are noise (observed swing ±3–5 claims); only the paired aggregate is load-bearing.
- **Safety:** zero NEW forbidden assertions outside the family (none present in flag runs that are absent in baseline runs); non-family claims delta not significantly negative under McNemar.
- **Adoption:** ≥50% of eligible unprompted sessions emit the frontmatter, OR D04 must add an assisted-authoring step (write-time service hint via the `supersession_warnings` channel suggesting a supersedes edge) before any Tier C reliance. The oracle-labeled retrieval result does not transfer without this.
- Overfetch guard: flag-arm context chars/case within 5% of baseline arm.

## Cost preflight and ceiling

Subscription rule (Decisions.md): all reasoning runs via the ChatGPT-authenticated Codex subscription, fail-closed — `require_codex_subscription` rejects API keys; zero usage-billed reasoning permitted.

All-in equivalent cost ≈ $0.24/agent-run (470-run audit, $113.18).

- Claim-scored runs: 3 arms × 14 cases × 3 draws = 126 runs.
- Adoption sessions: 12 × 3 draws = 36 runs.
- Total 162 runs × $0.24 = **$38.88**. Regrade passes ≈ $0 (no regeneration).
- Optional service-only draws 4-5: 2 arms × 14 cases × 2 draws = 56 runs =
  **$13.44**. The five-draw path totals 218 runs = **$52.32**; filesystem and
  adoption are not repeated.

Embeddings (usage-billed OpenAI, explicitly exempt, listed separately): none required — all arms run exact+lexical; no semantic-ready profile exists. If semantic indexing is later added to the fixture, cost ≤ $0.19 (9.6M-token corpus rate; this corpus is a fraction of that).

**Hard ceiling: $60** all-in equivalent. The optional five-draw path leaves
$7.68 of infrastructure-rerun headroom.

## Abort criteria

- Any draw shows the flag arm asserting stale facts *outside* the family at a higher rate than baseline → stop, flag off, report before any rerun.
- The definitive flag-on
  `results/2026-MM-DD-e07-supersession-write-latency.json` artifact is red, or
  its 64K `concurrent_probe.write_p95_ms` exceeds 58.0ms → stop before
  reasoning; this is the 07-26 unbudgeted-bookkeeping signature.
- Any usage-billed reasoning call detected, or running total exceeds $60 → abort immediately.
- ≥2 harness failures (timeouts/errors) in a draw → invalidate that draw entirely, fix, rerun; never average a broken draw.

## Reporting

The run record must contain: git commit fingerprint; per-arm flag
configuration; manifest version and hash (recent-work-v0.3); dedup-family
label list; the calibration/contract SHA-256 and both definitive write-latency
artifact paths; all draw artifact paths
(`results/2026-MM-DD-e07-supersession-{base,flag,filesystem}-draw<N>.json` and
the adoption artifacts); the three-arm aggregate and, if triggered, the
separate service-only five-draw aggregate; per-case paired win/loss/tie table;
McNemar exact p and bootstrap CI; forbidden-assertion counts in/out of family
per arm; adoption rate with the eligible-session list and each emitted
frontmatter block; context chars/case per arm; total cost split into
subscription-equivalent and embeddings-exempt lines; explicit pass/fail
against each acceptance criterion.

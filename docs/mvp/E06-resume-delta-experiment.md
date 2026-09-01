# E06 — Resume Delta Experiment

Status: Complete — negative result; D03 rejected
Date: 2026-07-27
Gates: D03 (D03-resume-delta-packets.md)
Phase: 1 (requires D03 built behind resume_deltas)

## Result (2026-07-28)

The definitive three-draw experiment rejected D03 in its tested form.
Treatment B passed every deterministic performance, SQL-count, footprint, and
lineage gate, including 77.606 ms resume p95 at 640K, exactly +5 completed SQL
statements in all 30 paired samples, and 30/30 byte/version-exact lineage
responses. It did not pass the quality or payload gates:

- B produced 0/5 complete cases in all three draws.
- B scored 34/60 claims versus A's 33/60 and C's 40/60. The one-sided exact
  claim-level McNemar p-values were 0.8125 versus A and 0.96875 versus C.
- B increased the operation-level resume payload in all 15/15 paired
  draw-case comparisons, by 63,387 characters total and 4,225.8 characters
  per pair on average.

`resume_deltas` remains default-off and is not eligible for rollout. The
definitive artifacts, complete card grid, cost record, and excluded-diagnostic
history are in
[the E06 result report](../../results/2026-07-28-e06-report.md).

## Question

Do resume delta packets — bounded before/after hydration of checkpoint sources that changed since the checkpoint — produce the first-ever transitions case win, and a paired improvement over both the current service_api_resume and the filesystem_rebuild control, while holding resume p95 ≤150ms at 640K?

Transitions are 0/5 in every run to date; failures are claim-slot omissions, never lineage loss. D03's mechanism targets exactly that omission class.

## Preconditions and build items

- B1 (M): IMPLEMENTED. D03 is behind `BRUNN_RESUME_DELTAS=false` by default. The resume-open path in `apps/api/src/simple_core.rs` performs the source_refs × changes_since_checkpoint intersection, one batched pinned/current history statement, whole_pair/unified_diff/lead materialization, integrity checks, and ≤6,000-char accounting against open evidence.
- B2 (S): IMPLEMENTED. `transition_eval.py --mutation-script` invokes `eval/e06_mutate.py` after the seed checkpoint exists. The hook applies deterministic, expected-version writes to exactly 3 author-ordered checkpoint paths and mirrors identical bytes into an isolated filesystem corpus. It reads both worlds before and after mutation and fails the draw on divergence. `eval/e06_mutations.json` and `eval/e06-sources/` are explicitly tagged synthetic fixtures.
- B3 (implemented): arm-aware n≥3 aggregator with optional one-sided, claim-ID-paired McNemar after strict-majority draw collapse; default case-level output remains two-sided. See [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
- B4 (implemented): the shared D09/performance harness reports definitive 30-sample resume p95 and query counts; the resume-delta gate is 150ms when the flag is on.
- Note: transition_eval seeds currently accept only workspace checkpoints; the writable-sidecar extension (Medium) is NOT required here — filesystem_rebuild is an existing runnable condition — but B2's vault mirror is mandatory for arm fairness.

**Resolved nuisance posture (2026-07-28):** E02 rejected D02, so every E06
service and performance stack must start with
`BRUNN_VERBATIM_SPANS=false` and every measured service arm must assert
`--expect-feature-flag verbatim_spans=off`. Verbatim spans are not an E06
variable. An E06 pass cannot rehabilitate D02.

## Arms

- A: service_api_resume, resume_deltas off (current behavior — baseline).
- B: service_api_resume, resume_deltas on (D03 under test).
- C: filesystem_rebuild (control; a file tree categorically lacks version history and a generation log, so it establishes the file ceiling for this task).

## Corpus and fixtures

- Transitions suite: 5 cards / 20 claims (worst chronic card: brunn-api-gate-transition). Model gpt-5.6-sol from manifest.
- Mutation set per card: 3 checkpoint source_refs paths, deterministic seeded edits that change facts the resuming agent must cite (e.g. a moved deadline, a changed gate threshold, a renamed owner). Sizing must exercise both D03 modes: at least one source ≤2,400 chars/version (whole_pair) and at least one larger (unified_diff) per card.
- Embeddings: --embeddings hashing for all arms (semantic stays off the critical path; zero usage-billed spend).
- Performance: 640k synthetic corpus via performance_eval --future-soak, 30 samples definitive.

## Procedure

1. Land B1–B4 on a clean git tree and allocate separate project-scoped
   resume-off and resume-on stacks.
2. Validate each deterministic mutation plan before any model run:
   `python3 transition_eval.py --manifest eval/transition_cases.json validate --mutation-script eval/e06_mutate.py --mutation-seed "e06-draw${N}"`.
   Validation requires exactly 3 unique author-ordered checkpoint paths per
   card, at least one source that remains ≤2,400 chars/version for
   `whole_pair`, and at least one larger source for `unified_diff`.
3. Run the cheap paired 640K performance control/treatment before reasoning.
   On the resume-off stack:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --verbatim-feature-acceptance not-applicable --query-budget-profile default-safe --exercise-resume-delta-fixture --label e06-resume-control --future-soak --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag resume_deltas=off --out results/2026-MM-DD-e06-resume-control.json`.
   Then, against the same image revision on the isolated resume-on stack:
   `python3 performance_eval.py run --gate-profile d03-resume-deltas --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --verbatim-feature-acceptance not-applicable --query-budget-profile d03-resume-deltas --query-budget-contract eval/query_budgets.d03-resume-deltas.json --resume-control-from results/2026-MM-DD-e06-resume-control.json --exercise-resume-delta-fixture --label e06-resume-treatment --future-soak --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag resume_deltas=on --out results/2026-MM-DD-e06-resume-treatment.json`.
   The shared fixture checkpoints the 640K target source, mutates that exact
   source, and verifies the new version before sampling. The treatment
   artifact must prove 640K resume p95 ≤150ms and exactly +5 completed SQL
   statements in every paired resume sample: context validation, context
   setup, timeout setup, one batched version-pair `SELECT`, and `COMMIT`. It
   must also prove all 30 treatment responses contain the exact byte-verified
   pinned/current `whole_pair`, while all 30 flag-off control responses omit
   `resume_deltas`. Any red gate stops the reasoning grid.
4. For draw N in 1..3, run the three arms with identical mutation seeds per
   card. Every service arm binds the actual running container and injects only
   exact+lexical retrieval; the harness proves the container remains running
   and unchanged before/after and that its OCI revision label equals `$REV`.
   - Arm A (isolated stack with `BRUNN_RESUME_DELTAS=false`):
     `python3 transition_eval.py --manifest eval/transition_cases.json run --condition service_api_resume --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --experiment-arm e06-A --paired-draw-id "e06-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=off --expect-feature-flag intention_ledger=off --expect-feature-flag resume_deltas=off --embeddings hashing --mutation-script eval/e06_mutate.py --mutation-seed "e06-draw${N}" --run-id "resume-deltas-a-draw${N}" --out "results/2026-MM-DD-resume-deltas-a-draw${N}.json" --report "results/2026-MM-DD-resume-deltas-a-draw${N}.md"`.
   - Arm B (separate isolated stack with `BRUNN_RESUME_DELTAS=true`):
     `python3 transition_eval.py --manifest eval/transition_cases.json run --condition service_api_resume --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --experiment-arm e06-B --paired-draw-id "e06-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=off --expect-feature-flag intention_ledger=off --expect-feature-flag resume_deltas=on --embeddings hashing --mutation-script eval/e06_mutate.py --mutation-seed "e06-draw${N}" --run-id "resume-deltas-b-draw${N}" --out "results/2026-MM-DD-resume-deltas-b-draw${N}.json" --report "results/2026-MM-DD-resume-deltas-b-draw${N}.md"`.
     Never flip the flag on a stack serving another concurrent arm.
   - Arm C (filesystem-only, so no service/image/runtime arguments):
     `python3 transition_eval.py --manifest eval/transition_cases.json run --condition filesystem_rebuild --experiment-arm e06-C --paired-draw-id "e06-draw${N}" --embeddings hashing --mutation-script eval/e06_mutate.py --mutation-seed "e06-draw${N}" --run-id "resume-deltas-c-draw${N}" --out "results/2026-MM-DD-resume-deltas-c-draw${N}.json" --report "results/2026-MM-DD-resume-deltas-c-draw${N}.md"`.
5. Aggregate with B first because the directional alternative is load-bearing:
   `E06_MAIN=(results/2026-MM-DD-resume-deltas-{a,b,c}-draw{1,2,3}.json); python3 eval/aggregate_draws.py "${E06_MAIN[@]}" --expected-arm e06-B --expected-arm e06-A --expected-arm e06-C --expected-arm-retrieval-modes e06-B=exact,lexical --expected-arm-retrieval-modes e06-A=exact,lexical --claim-mcnemar-alternative a_greater --out results/2026-MM-DD-resume-deltas-aggregate.json`.
   This emits the required one-sided claim-level tests for B-vs-A and B-vs-C
   while retaining the default two-sided case-level McNemar and clustered
   bootstrap. The aggregate is valid only when every input is ledger-bound to
   the same mutation script, each arm in a paired draw uses a byte-identical
   mutation-plan set, and the mutation seed exactly equals that paired-draw ID.
   The aggregate records those checks under `mutation_provenance`.
6. Audit the operation-level resume payloads from exactly the three definitive
   A draws and three definitive B draws:
   `E06_RESUME_PAYLOADS=(results/2026-MM-DD-resume-deltas-{a,b}-draw{1,2,3}.json); python3 eval/audit_resume_payloads.py "${E06_RESUME_PAYLOADS[@]}" --manifest eval/transition_cases.json --out results/2026-MM-DD-e06-resume-payload-comparison.json`.
   The auditor revalidates the immutable run, source, image, retrieval-mode,
   runtime-feature, mutation-plan, mutation-receipt, and ledger bindings. It
   requires the exact 2-arm × 3-draw × 5-case grid and exactly one successful,
   uncontaminated `service_operations` entry whose `operation` is `resume` per
   record. It compares only that operation's `result_chars`, never whole-card
   response metrics. Every pair must have treatment `result_chars` ≤ control `result_chars`,
   with zero tolerance; any malformed or drifted evidence fails
   closed and emits a machine-readable failing artifact.
7. If the headline is inside the noise floor but brunn-api-gate-transition moves, run a targeted 5-draw repeat on that card, all three arms, adding `--case brunn-api-gate-transition` to every invocation and using targeted-specific paired-draw IDs.

   **Predeclared for the 2026-07-28 execution:** do not invoke this optional
   repeat. Neither "inside the noise floor" nor "moves" has a frozen numeric
   definition, and the targeted paired-draw IDs, output ledger, and aggregate
   are unspecified. The three-draw five-card main grid is definitive for this
   run. Any targeted follow-up requires a new predeclared plan before looking
   at regenerated evidence.
8. Use `python3 transition_eval.py --manifest eval/transition_cases.json regrade ...` for rubric corrections; never regenerate answers to fix grading.

The run JSON fingerprints the mutation script, embeds every per-card plan and
receipt, hash-binds the complete mutation evidence in the immutable run ledger,
records the feature-flag state, and uses the mutated authority path in
grading/lineage checks. Definitive aggregation revalidates exact case coverage,
three-source plans, target bytes/hashes, and service/filesystem receipt
semantics. The control sees only its prior checkpoint plus the post-mutation
file tree; it is not handed a synthetic change-log file.

## Metrics

- Claims per arm per draw (n/20); per-card win/loss/tie grids B-vs-A and B-vs-C.
- Cases won per arm per draw (historical baseline: 0/5 everywhere); brunn-api-gate-transition tracked individually.
- Resume p95 at 640k with flag on, vs the 35.2ms v8 baseline (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json) and the 150ms gate.
- Operation-level resume `result_chars`, arm B vs arm A, paired by draw and
  case. Whole-card response metrics are ineligible for this budget check.
- Query count per resume open (must be exactly +5 completed statements in arm
  B, representing one batched application `SELECT` plus four authenticated
  transaction statements).

## Acceptance criteria

- First-ever transitions case win: arm B scores >0/5 cases in at least 2 of 3 draws.
- Paired improvement over BOTH arms: claim-level exact McNemar favors B over A and B over C (alpha 0.05, one-sided given the directional hypothesis), with the direction consistent in all 3 draws. With only 20 claims per draw, the paired aggregate across draws is the load-bearing statistic; single-draw deltas are noise per the ±3-5 claim floor.
- Resume p95 ≤150ms at the 640k soak (~4x the 35.2ms baseline; the loose 500ms figure is rejected per D03).
- Checkpoint write path unchanged: footprint gate (≤100 rows/4MiB; actual 11 rows/~55KB) and protocol-to-evidence ratio ≤1.0 both hold.
- Payload budget-neutrality: in every one of the 15 definitive A/B draw-case
  pairs, the single successful `service_operations` resume entry has treatment
  `result_chars` ≤ control `result_chars`, with zero tolerance.

## Cost preflight and ceiling

Subscription rule: all reasoning runs via the ChatGPT-authenticated Codex subscription, fail-closed (require_codex_subscription rejects API keys).

- Main grid: 3 arms × 5 cards × 3 draws = 45 case-runs × $0.24 = $10.80.
- Targeted repeat worst case: 3 arms × 1 card × 5 draws = 15 × $0.24 = $3.60.
- Preflight total: $14.40.
- Embeddings-exempt spend: $0 planned (--embeddings hashing throughout; an openai-embeddings arm is out of scope and would cost ~$0.19 per 9.6M-token corpus if ever added — listed separately).
- Hard ceiling: $30. Stop all runs at the ceiling regardless of state.

## Abort criteria

- Any checkpoint-lineage incident — parent_checkpoint_id fails to resolve, pinned-version sha256 mismatch, or a delta pairs the wrong versions: immediate abort of the experiment and flag-off, mirroring the Tier C lineage tripwire.
- Workspace/vault mutation divergence detected in any draw: invalidate that draw, fix B2, restart the draw (all arms).
- Any reasoning run bills an API key (fail-closed breach): abort, file defect.
- Any D03 deterministic gate fails (150ms p95, paired exact +5 completed
  statements, matched fixture identity, or byte-verified response lineage):
  abort before reasoning; do not average the failure away.
- Spend reaches $30.

## Reporting

The run record must contain: git fingerprint with clean-tree confirmation; flag state and mutation seeds per draw; per-draw per-arm JSON and MD artifact paths; the full per-card grid including brunn-api-gate-transition; paired McNemar p-values and CIs for B-vs-A and B-vs-C; cases-won table against the historical 0/5; soak numbers (resume p95, concurrent probe, footprint, protocol-to-evidence, query counts); `results/2026-MM-DD-e06-resume-payload-comparison.json` with all 15 operation-level control/treatment `result_chars` pairs, per-pair deltas, totals, means, and the zero-tolerance verdict; cost actuals vs preflight with subscription and embeddings-exempt spend listed separately.

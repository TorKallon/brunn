# E06 — Resume Delta Experiment

Status: Build prerequisites implemented — experiment not run
Date: 2026-07-27
Gates: D03 (D03-resume-delta-packets.md)
Phase: 1 (requires D03 built behind resume_deltas)

## Question

Do resume delta packets — bounded before/after hydration of checkpoint sources that changed since the checkpoint — produce the first-ever transitions case win, and a paired improvement over both the current service_api_resume and the filesystem_rebuild control, while holding resume p95 ≤150ms at 640K?

Transitions are 0/5 in every run to date; failures are claim-slot omissions, never lineage loss. D03's mechanism targets exactly that omission class.

## Preconditions and build items

- B1 (M): IMPLEMENTED. D03 is behind `STRAYLIGHT_RESUME_DELTAS=false` by default. The resume-open path in `apps/api/src/simple_core.rs` performs the source_refs × changes_since_checkpoint intersection, one batched pinned/current history statement, whole_pair/unified_diff/lead materialization, integrity checks, and ≤6,000-char accounting against open evidence.
- B2 (S): IMPLEMENTED. `transition_eval.py --mutation-script` invokes `eval/e06_mutate.py` after the seed checkpoint exists. The hook applies deterministic, expected-version writes to exactly 3 author-ordered checkpoint paths and mirrors identical bytes into an isolated filesystem corpus. It reads both worlds before and after mutation and fails the draw on divergence. `eval/e06_mutations.json` and `eval/e06-sources/` are explicitly tagged synthetic fixtures.
- B3 (S): n≥3 paired-draw aggregator eval/aggregate_draws.py (per-case win/loss/tie, exact-binomial McNemar, bootstrap CIs, stdlib only) — known build item, shared with E02 (E02-verbatim-identifier-gate.md), specified in E01-paired-draw-machinery-and-baseline.md.
- B4 (S): INTEGRATION DEPENDENCY. The definitive 30-sample resume p95 and query-count reporting belong to the shared D09/performance harness and must be present in the clean integrated execution fingerprint before E06 is run.
- Note: transition_eval seeds currently accept only workspace checkpoints; the writable-sidecar extension (Medium) is NOT required here — filesystem_rebuild is an existing runnable condition — but B2's vault mirror is mandatory for arm fairness.

## Arms

- A: service_api_resume, resume_deltas off (current behavior — baseline).
- B: service_api_resume, resume_deltas on (D03 under test).
- C: filesystem_rebuild (control; a file tree categorically lacks version history and a generation log, so it establishes the file ceiling for this task).

## Corpus and fixtures

- Transitions suite: 5 cards / 20 claims (worst chronic card: straylight-api-gate-transition). Model gpt-5.6-sol from manifest.
- Mutation set per card: 3 checkpoint source_refs paths, deterministic seeded edits that change facts the resuming agent must cite (e.g. a moved deadline, a changed gate threshold, a renamed owner). Sizing must exercise both D03 modes: at least one source ≤2,400 chars/version (whole_pair) and at least one larger (unified_diff) per card.
- Embeddings: --embeddings hashing for all arms (semantic stays off the critical path; zero usage-billed spend).
- Performance: 640k synthetic corpus via performance_eval --future-soak, 30 samples definitive.

## Procedure

1. Land B1–B4 on a clean git tree (implementation fingerprint gate).
2. Validate the deterministic mutation plans: `python3 transition_eval.py validate --mutation-script eval/e06_mutate.py --mutation-seed e06-draw<N>`. Validation requires exactly 3 unique author-ordered checkpoint paths per card, at least one source that remains ≤2,400 chars/version for `whole_pair`, and at least one larger source for `unified_diff`. Runtime receipts prove workspace/vault byte equality.
3. For draw N in 1..3, run the three arms with identical mutation seeds per card:
   - Arm A (isolated stack with `STRAYLIGHT_RESUME_DELTAS=false`): `python3 transition_eval.py run --condition service_api_resume --service-protocol simple --embeddings hashing --mutation-script eval/e06_mutate.py --mutation-seed e06-draw<N> --run-id resume-deltas-a-draw<N> --out results/2026-MM-DD-resume-deltas-a-draw<N>.json --report results/2026-MM-DD-resume-deltas-a-draw<N>.md`
   - Arm B (separate isolated stack with `STRAYLIGHT_RESUME_DELTAS=true`): same command with slug `resume-deltas-b-draw<N>`. Never flip the flag on a stack serving another concurrent arm.
   - Arm C: `python3 transition_eval.py run --condition filesystem_rebuild --service-protocol simple --embeddings hashing --mutation-script eval/e06_mutate.py --mutation-seed e06-draw<N> --run-id resume-deltas-c-draw<N> --out results/2026-MM-DD-resume-deltas-c-draw<N>.json --report results/2026-MM-DD-resume-deltas-c-draw<N>.md`
4. Aggregate with B3: `python eval/aggregate_draws.py results/2026-MM-DD-resume-deltas-*-draw*.json --out results/2026-MM-DD-resume-deltas-aggregate.json` — per-card pairing B-vs-A and B-vs-C across the 3 draws; exact McNemar at claim level; bootstrap CIs at card level.
5. `python performance_eval.py run --label resume-deltas-soak --future-soak --out results/2026-MM-DD-resume-deltas-soak.json` with resume_deltas on; read resume p95, concurrent write/search probe, checkpoint footprint, protocol-to-evidence ratio, and the query-count assertion.
6. If the headline is inside the noise floor but straylight-api-gate-transition moves, run a targeted 5-draw repeat on that card, all three arms.
7. Use `transition_eval.py regrade` for rubric corrections; never regenerate answers to fix grading.

The run JSON fingerprints the mutation script, embeds every per-card plan and receipt, records the feature-flag state, and uses the mutated authority path in grading/lineage checks. The control sees only its prior checkpoint plus the post-mutation file tree; it is not handed a synthetic change-log file.

## Metrics

- Claims per arm per draw (n/20); per-card win/loss/tie grids B-vs-A and B-vs-C.
- Cases won per arm per draw (historical baseline: 0/5 everywhere); straylight-api-gate-transition tracked individually.
- Resume p95 at 640k with flag on, vs the 35.2ms v8 baseline (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json) and the 150ms gate.
- Open payload chars per resume, arm B vs arm A (budget-neutrality check: deltas are charged against the evidence budget, so totals must not grow beyond noise).
- Query count per resume open (must be exactly +1 batched round trip in arm B).

## Acceptance criteria

- First-ever transitions case win: arm B scores >0/5 cases in at least 2 of 3 draws.
- Paired improvement over BOTH arms: claim-level exact McNemar favors B over A and B over C (alpha 0.05, one-sided given the directional hypothesis), with the direction consistent in all 3 draws. With only 20 claims per draw, the paired aggregate across draws is the load-bearing statistic; single-draw deltas are noise per the ±3-5 claim floor.
- Resume p95 ≤150ms at the 640k soak (~4x the 35.2ms baseline; the loose 500ms figure is rejected per D03).
- Checkpoint write path unchanged: footprint gate (≤100 rows/4MiB; actual 11 rows/~55KB) and protocol-to-evidence ratio ≤1.0 both hold.
- Payload budget-neutrality: arm B open chars within noise of arm A.

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
- Resume p95 exceeds 500ms mid-experiment (grossly off the 150ms gate): abort the perf claim, continue reasoning draws only if lineage is intact.
- Spend reaches $30.

## Reporting

The run record must contain: git fingerprint with clean-tree confirmation; flag state and mutation seeds per draw; per-draw per-arm JSON and MD artifact paths; the full per-card grid including straylight-api-gate-transition; paired McNemar p-values and CIs for B-vs-A and B-vs-C; cases-won table against the historical 0/5; soak numbers (resume p95, concurrent probe, footprint, protocol-to-evidence, query counts); open payload char comparison; cost actuals vs preflight with subscription and embeddings-exempt spend listed separately.

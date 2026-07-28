# E08 — Intention Ledger Experiment

Status: Specified — not run
Date: 2026-07-27
Gates: D05 (D05-intention-ledger.md)
Phase: 1 (requires flagged feature build)

## Question

Does surfacing ≤5 pointer-only pending intentions at open (D05-intention-ledger.md) recover prospective-memory claim slots, without false surfacing, without regressing non-prospective cases, and without measurable open latency cost?

## Preconditions and build items

1. D05 implemented behind `intention_ledger` — **M**. Anchors: apps/api/src/simple_core.rs open path (the lane-dispatch site that assembles the open response), frontmatter parse shared with D04.
2. Corpus seeding: 6 intention notes added to the shared fixture corpus, ≤2 relevant to any single case — **S** (fixture files with exact paths and sha256; frontmatter authored in Markdown per the round-trip rule).
3. Two new manifest cases (shapes under Corpus and fixtures) added to eval/recent_work_cases.json, making recent-work-v0.3; rubric claims written; `validate` re-run — **M**.
4. Prospective subset manifest eval/e08_prospective_cases.json assembling the 5 prospective case definitions (3 chronic + 2 new) against the same fixtures, so the filesystem control and prospective-focused runs don't pay for full suites — **S**.
5. Harness flag/arm plumbing and authenticated runtime snapshots — implemented; see [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
6. Arm-aware n≥3 paired-draw aggregator — implemented.
7. False-surfacing audit script scanning saved open responses in run artifacts against the case oracle — implemented as `eval/audit_intentions.py`.
8. Adoption measurement — implemented and shared with E07 arm 4; do not duplicate.

**Resolved nuisance posture (2026-07-28):** E02 rejected D02, so every E08
service and performance stack must start with
`STRAYLIGHT_VERBATIM_SPANS=false` and every measured service arm must assert
`--expect-feature-flag verbatim_spans=off`. Verbatim spans are not an E08
variable. An E08 pass cannot rehabilitate D02.

## Arms

1. **service_api flag-off** (baseline).
2. **service_api flag-on**.
3. **filesystem** — read-only condition; intention notes present as ordinary files. Control question: can a file-browsing agent find the obligations unaided?

Primary paired contrast is arm 1 vs arm 2; arm 3 is the reference control on the prospective subset only.

## Corpus and fixtures

Base corpus: recent-work-v0.3 (E07's v0.3 plus the two new cases) with personal-coordination fixtures available for the coord case.

Seeded intentions (6 total):
- ≤2 relevant to any single prospective case.
- ≥2 relevant to no case at all (false-surfacing probes with plausible trigger terms).
- 1 with `status: done` — must never surface anywhere.
- 1 overdue (`due` past, still pending) — must surface as `overdue`, never silently dropped, never asserted as future.

Prospective cases (5, in eval/e08_prospective_cases.json):
- recent-aether-gmail-actions, recent-aether-morning-brief (eval/recent_work_cases.json), coord-deadline-readiness (eval/personal_coordination_cases.json).
- **New case A — cross-agent surfacing (4 claims):** the intention was authored under a different agent identity/session in fixture history; the task is an unrelated-looking request whose open queries hit the trigger domain. Claims: acts on the obligation; cites the intention note's exact path; states the correct due date; invents no details beyond the note.
- **New case B — expiry/negative (4 claims):** the task domain matches both the done and the overdue seeded intentions. Claims include forbidden assertions: must NOT present the done intention as pending; must flag the overdue one as overdue; must not assert it as a future-dated obligation; must still complete the base task.

5 cases × 4 claims = 20 prospective slots per draw. The full recent-work-v0.3 suite runs in both service arms for the no-regression gate and to maximize opens for the false-surfacing denominator.

## Procedure

1. Preflight: use separate project-scoped stacks from
   [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md), record
   the clean immutable build revision, and confirm flags default off.
2. `python3 agent_work_eval.py --manifest eval/recent_work_cases.json validate` and `python3 agent_work_eval.py --manifest eval/e08_prospective_cases.json validate` — both must pass.
3. Calibrate the flag-on query shape before its acceptance run:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile calibration --label e08-query-budget-calibration --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag intention_ledger=on --out results/2026-MM-DD-e08-query-budget-calibration.json`.
   This command intentionally exits nonzero because calibration artifacts are
   never acceptance evidence. Review all 30 samples per operation. Author the
   contract only from those observed counts, use schema
   `straylight-query-budgets@v1`, set profile `e08-intention-ledger`, and copy
   the authenticated calibration runtime-feature applicability into the
   contract. Do not copy thresholds from `default-safe` or invent unmeasured
   headroom. Freeze and validate the reviewed artifact:
   `E08_QUERY_BUDGET_CONTRACT="results/2026-MM-DD-e08-intention-ledger-query-budgets.json"; test -s "$E08_QUERY_BUDGET_CONTRACT"; python3 -m json.tool "$E08_QUERY_BUDGET_CONTRACT" >/dev/null; python3 -c 'import json,sys; p=json.load(open(sys.argv[1])); assert p["schema"]=="straylight-query-budgets@v1"; assert p["profile"]=="e08-intention-ledger"; assert p["runtime_features"]["intention_ledger"] is True; assert p["operations"]' "$E08_QUERY_BUDGET_CONTRACT"; E08_QUERY_BUDGET_SHA256="$(shasum -a 256 "$E08_QUERY_BUDGET_CONTRACT" | awk '{print $1}')"; test -n "$E08_QUERY_BUDGET_SHA256"; chmod 0444 "$E08_QUERY_BUDGET_CONTRACT"`.
   Record the reviewer, calibration artifact SHA-256, contract SHA-256, and
   review decision. Any later contract change requires a new path, hash, and
   calibration review.
4. Run both cheap 64K latency arms before reasoning. Flag off:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile default-safe --label e08-open-latency-off --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag intention_ledger=off --out results/2026-MM-DD-e08-open-latency-off.json`.
   Flag on, against its isolated stack:
   `python3 performance_eval.py run --protocol simple --retrieval-modes exact lexical --semantic-failure-probe not-applicable --query-budget-profile e08-intention-ledger --query-budget-contract "$E08_QUERY_BUDGET_CONTRACT" --label e08-open-latency-on --scales 64000 --samples 30 --api-container "$API_CONTAINER" --db-container "$DB_CONTAINER" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag intention_ledger=on --out results/2026-MM-DD-e08-open-latency-on.json`.
   Confirm the on artifact's `query_budget_contract.sha256` equals
   `E08_QUERY_BUDGET_SHA256`. Any red gate or open p95 delta ≥10ms stops all
   reasoning.
5. For draw N in 1..3:
   1. Full-suite base:
      `python3 agent_work_eval.py --manifest eval/recent_work_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e08-base --paired-draw-id "e08-full-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=off --expect-feature-flag intention_ledger=off --expect-feature-flag resume_deltas=off --concurrency 3 --timeout 360 --run-id "e08-base-full-run${N}" --out "results/2026-MM-DD-e08-intention-base-draw${N}.json" --report "results/2026-MM-DD-e08-intention-base-draw${N}.md"`.
   2. Full-suite flag:
      `python3 agent_work_eval.py --manifest eval/recent_work_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e08-flag --paired-draw-id "e08-full-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=off --expect-feature-flag intention_ledger=on --expect-feature-flag resume_deltas=off --concurrency 3 --timeout 360 --run-id "e08-flag-full-run${N}" --out "results/2026-MM-DD-e08-intention-flag-draw${N}.json" --report "results/2026-MM-DD-e08-intention-flag-draw${N}.md"`.
   3. Prospective base:
      `python3 agent_work_eval.py --manifest eval/e08_prospective_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e08-base --paired-draw-id "e08-prospective-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=off --expect-feature-flag intention_ledger=off --expect-feature-flag resume_deltas=off --concurrency 3 --timeout 360 --run-id "e08-base-prospective-run${N}" --out "results/2026-MM-DD-e08-prospective-base-draw${N}.json" --report "results/2026-MM-DD-e08-prospective-base-draw${N}.md"`.
   4. Prospective flag:
      `python3 agent_work_eval.py --manifest eval/e08_prospective_cases.json run --service-protocol simple --service-retrieval-modes exact lexical --api-container "$API_CONTAINER" --condition service_api --experiment-arm e08-flag --paired-draw-id "e08-prospective-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag verbatim_spans=off --expect-feature-flag supersession_demotion=off --expect-feature-flag intention_ledger=on --expect-feature-flag resume_deltas=off --concurrency 3 --timeout 360 --run-id "e08-flag-prospective-run${N}" --out "results/2026-MM-DD-e08-prospective-flag-draw${N}.json" --report "results/2026-MM-DD-e08-prospective-flag-draw${N}.md"`.
   5. Prospective filesystem control, with no service runtime expectations:
      `python3 agent_work_eval.py --manifest eval/e08_prospective_cases.json run --condition filesystem --experiment-arm e08-filesystem --paired-draw-id "e08-prospective-draw${N}" --concurrency 3 --timeout 360 --run-id "e08-filesystem-prospective-run${N}" --out "results/2026-MM-DD-e08-prospective-filesystem-draw${N}.json" --report "results/2026-MM-DD-e08-prospective-filesystem-draw${N}.md"`.
6. False-surfacing audit uses only exact flag-on draw artifacts:
   `E08_FLAG=(results/2026-MM-DD-e08-intention-flag-draw{1,2,3}.json results/2026-MM-DD-e08-prospective-flag-draw{1,2,3}.json); python3 eval/audit_intentions.py "${E08_FLAG[@]}" --full-manifest eval/recent_work_cases.json --prospective-manifest eval/e08_prospective_cases.json --expected-full-draw e08-full-draw1 --expected-full-draw e08-full-draw2 --expected-full-draw e08-full-draw3 --expected-prospective-draw e08-prospective-draw1 --expected-prospective-draw e08-prospective-draw2 --expected-prospective-draw e08-prospective-draw3 --out results/2026-MM-DD-e08-intention-audit.json`.
   The audit hashes and reopens exactly six raw artifacts, validates their
   manifest/source/build/image/runtime/feature/mode/arm/draw provenance, and
   exits nonzero on either invalid evidence or a failed audit gate.
7. Regrade disputed answers with the correct global `--manifest` before
   `regrade`. Never put original and regraded versions of one draw into the
   same aggregate. Aggregate the two case/arm sets separately:
   `E08_FULL=(results/2026-MM-DD-e08-intention-{flag,base}-draw{1,2,3}.json); python3 eval/aggregate_draws.py "${E08_FULL[@]}" --expected-arm e08-flag --expected-arm e08-base --expected-arm-retrieval-modes e08-flag=exact,lexical --expected-arm-retrieval-modes e08-base=exact,lexical --require-feature-family prospective --out results/2026-MM-DD-e08-full-aggregate.json`.
   Then aggregate the prospective subset:
   `E08_PROSPECTIVE=(results/2026-MM-DD-e08-prospective-{flag,base,filesystem}-draw{1,2,3}.json); python3 eval/aggregate_draws.py "${E08_PROSPECTIVE[@]}" --expected-arm e08-flag --expected-arm e08-base --expected-arm e08-filesystem --expected-arm-retrieval-modes e08-flag=exact,lexical --expected-arm-retrieval-modes e08-base=exact,lexical --out results/2026-MM-DD-e08-prospective-aggregate.json`.

## Metrics

- Prospective slots /20 per arm per draw (from subset runs).
- False-surfacing rate: irrelevant `pending_intentions` items ÷ total opens across flag-on runs. Done-intention appearing anywhere counts as an automatic failure, not a rate contribution.
- Non-prospective regression: paired per-case win/loss/tie on all non-prospective recent-work-v0.3 cases in the same draws; McNemar.
- Open p95 delta at 64k (flag on − off).
- `pending_intentions` char count per open (≤500 assertion).

## Acceptance criteria

- **Primary:** flag-on beats flag-off by net ≥ +6 prospective slot-instances summed across ≥3 paired draws (of 60 slot-instances), McNemar exact p and bootstrap CI reported. Single-draw deltas are noise (±3–5 claims observed swing).
- **False surfacing < 10%**, and zero surfacings of the `status: done` fixture, and case B's forbidden assertions all held (zero stale-pending or future-dating assertions in flag-on runs).
- **No regression:** non-prospective paired delta not significantly negative under McNemar; zero new forbidden assertions on non-prospective cases.
- **Latency:** open p95 delta < 10ms at 64k; the ≤500-char assertion never violated.
- **Adoption confound (shared with E07):** oracle-seeded intentions overstate deployed benefit exactly as E07's seeded frontmatter does. E08 passes only alongside the shared adoption measurement (E07 arm 4 instrument): ≥50% unprompted authoring on eligible sessions, or D05 ships with an assisted-authoring step before Tier C reliance.

## Cost preflight and ceiling

Subscription rule (Decisions.md): all reasoning via the ChatGPT-authenticated Codex subscription, fail-closed (`require_codex_subscription` rejects API keys); zero usage-billed reasoning.

All-in equivalent ≈ $0.24/agent-run (470-run audit, $113.18).

- Full-suite service runs: 14 cases × 2 arms × 3 draws = 84.
- Prospective subset service runs: 5 × 2 × 3 = 30.
- Filesystem subset runs: 5 × 3 = 15.
- Total 129 runs × $0.24 ≈ **$30.96**. Regrade ≈ $0; performance_eval runs involve no reasoning model, $0.

Embeddings (usage-billed OpenAI, exempt, listed separately): none —
exact+lexical only, semantic lane explicitly disabled, and no worker. $0.00.

**Hard ceiling: $40** all-in equivalent; ~$9 headroom covers one invalidated-draw rerun.

## Abort criteria

- Draw 1 false-surfacing rate > 25%, or any `status: done` surfacing → stop, fix trigger matching or the projection, restart the experiment (do not keep partial draws).
- Open p95 delta ≥10ms at 64k → stop; latency design assumption is wrong.
- Any usage-billed reasoning call detected, or running total > $40 → abort immediately.
- ≥2 harness failures in a draw → invalidate the draw, fix, rerun; never average a broken draw.

## Reporting

The run record must contain: git commit fingerprint; per-arm flag config;
manifest versions and hashes (recent-work-v0.3, e08_prospective,
personal_coordination); the calibration path/hash, reviewer decision, frozen
query-contract path/hash, and both latency artifacts; the 6 seeded intention
paths with frontmatter; every exact full/prospective/filesystem artifact path;
both aggregate paths; prospective slot table per arm per draw with paired
aggregate, McNemar p, and bootstrap CI; false-surfacing audit path and every
flagged instance; non-prospective paired table; open p95 off/on with delta;
char-assertion results; cost split (subscription-equivalent vs
embeddings-exempt); adoption measurement reference (E07 artifact) and its
result; explicit pass/fail per acceptance criterion.

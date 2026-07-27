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
5. Harness flag plumbing per arm — **S** (shared with E07-supersession-experiment.md; build once).
6. n≥3 paired-draw aggregator eval/aggregate_draws.py (McNemar, bootstrap CIs, stdlib only) — **S** (shared build item with E07; specified in E01-paired-draw-machinery-and-baseline.md).
7. False-surfacing audit script scanning saved open responses in run artifacts against the case oracle — **S**.
8. Adoption measurement — shared instrument with E07 arm 4; do not duplicate (**M** only if E07's harness is not yet built).

## Arms

1. **service_api flag-off** (baseline).
2. **service_api flag-on**.
3. **filesystem** — read-only condition; intention notes present as ordinary files. Control question: can a file-browsing agent find the obligations unaided?

Primary paired contrast is arm 1 vs arm 2; arm 3 is the reference control on the prospective subset only.

## Corpus and fixtures

Base corpus: recent-work-v0.3 (E07's v0.2 plus the two new cases) with personal-coordination fixtures available for the coord case.

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

1. Preflight: clean git tree, record commit; flags default off.
2. `python agent_work_eval.py validate --manifest eval/recent_work_cases.json` and `python agent_work_eval.py validate --manifest eval/e08_prospective_cases.json` — both must pass.
3. For draw N in 1..3:
   1. `python agent_work_eval.py run --manifest eval/recent_work_cases.json --condition service_api --concurrency 3 --timeout 360 --run-id e08-base-draw<N> --out results/2026-MM-DD-e08-intention-base-draw<N>.json --report results/2026-MM-DD-e08-intention-base-draw<N>.md` (flag off).
   2. Same with flag on: `--run-id e08-flag-draw<N>`, `--out results/2026-MM-DD-e08-intention-flag-draw<N>.json`.
   3. Prospective subset, both service arms: `--manifest eval/e08_prospective_cases.json`, artifacts `results/2026-MM-DD-e08-prospective-{base,flag}-draw<N>.json`.
   4. Filesystem control on the subset: `--condition filesystem`, `--out results/2026-MM-DD-e08-prospective-fs-draw<N>.json`.
4. Latency: `python performance_eval.py run --label e08-open-latency-off --scales 64000 --samples 30 --out results/2026-MM-DD-e08-open-latency-off.json` with the flag off, then the same with `--label e08-open-latency-on` and the flag on (embeddings pending, as in every baseline; no reasoning cost).
5. False-surfacing audit over all flag-on artifacts.
6. `regrade` disputed answers; then aggregate: `python eval/aggregate_draws.py results/2026-MM-DD-e08-*-draw*.json --out results/2026-MM-DD-e08-aggregate.json` on the paired base/flag draws.

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

Embeddings (usage-billed OpenAI, exempt, listed separately): none — exact+lexical only; performance_eval.py runs with embeddings pending (it has no embeddings flag; tests/mock_openai_embeddings.py provides a deterministic local server if a run ever insists on semantic coverage). $0.00.

**Hard ceiling: $40** all-in equivalent; ~$9 headroom covers one invalidated-draw rerun.

## Abort criteria

- Draw 1 false-surfacing rate > 25%, or any `status: done` surfacing → stop, fix trigger matching or the projection, restart the experiment (do not keep partial draws).
- Open p95 delta > 25ms at 64k → stop; latency design assumption is wrong.
- Any usage-billed reasoning call detected, or running total > $40 → abort immediately.
- ≥2 harness failures in a draw → invalidate the draw, fix, rerun; never average a broken draw.

## Reporting

The run record must contain: git commit fingerprint; per-arm flag config; manifest versions and hashes (recent-work-v0.3, e08_prospective, personal_coordination); the 6 seeded intention paths with frontmatter; all artifact paths per draw; prospective slot table per arm per draw with paired aggregate, McNemar p, bootstrap CI; false-surfacing rate with every flagged instance listed; non-prospective paired table; open p95 off/on with delta; char-assertion results; cost split (subscription-equivalent vs embeddings-exempt); adoption measurement reference (E07 artifact) and its result; explicit pass/fail per acceptance criterion.

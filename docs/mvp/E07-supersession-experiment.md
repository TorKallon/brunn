# E07 — Supersession Demotion and Adoption Experiment

Status: Specified — not run
Date: 2026-07-27
Gates: D04 (D04-supersession-current-truth.md)
Phase: 1 (requires flagged feature build)

## Question

Does explicit `supersedes` frontmatter with demotion-plus-annotation (D04-supersession-current-truth.md) win the dedup/current-over-history family without harming anything else — and, separately, will agents actually author the frontmatter unprompted? The second question is load-bearing: an oracle-seeded corpus proves the mechanism, not the deployed behavior.

## Preconditions and build items

1. D04 implemented behind `supersession_demotion` — **M**. Anchors: apps/api/src/simple_core.rs scoring (the derived_penalty application site), write/import frontmatter parse path, read view parameter.
2. recent-work-v0.3 manifest: correction notes carry real `supersedes` frontmatter and `feature_families.supersession_dedup` freezes the four relevant cases — implemented.
3. Harness flag plumbing: per-arm runtime-config toggle of `supersession_demotion` from agent_work_eval.py run setup (env var or config endpoint per D04 rollout mechanism) — **S**.
4. Arm-aware n≥3 paired-draw aggregator and runtime snapshot contract — implemented; see [Experiment-run-infrastructure.md](Experiment-run-infrastructure.md).
5. Adoption-arm harness: scripted unprompted agent-work write sessions where the authoring contract is documented in workspace docs but never stated in the task prompt; an eligibility oracle labels which sessions *should* have emitted `supersedes` (session writes a note that factually corrects an existing one) — **M**.

## Arms

1. **service_api-baseline** — simplified core, flag off, v0.2 corpus (frontmatter present in source text but mechanically inert).
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
3. For draw N in 1..3 (minimum; extend to 5 if the aggregate is borderline):
   1. `python3 agent_work_eval.py --manifest eval/recent_work_cases.json run --service-protocol simple --condition service_api --experiment-arm e07-base --paired-draw-id "e07-draw${N}" --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag supersession_demotion=off --expect-runtime-config supersession_demotion_weight=1.5 --concurrency 3 --timeout 360 --run-id "e07-base-run${N}" --out "results/2026-MM-DD-e07-supersession-base-draw${N}.json" --report "results/2026-MM-DD-e07-supersession-base-draw${N}.md"`.
   2. Same paired-draw ID against the isolated flag stack with
      `--experiment-arm e07-flag`,
      `--expect-feature-flag supersession_demotion=on`, the same explicit
      weight `1.5`, a unique run ID, and the flag artifact.
   3. Filesystem uses `--condition filesystem --experiment-arm e07-filesystem --paired-draw-id e07-draw<N>` and a unique run ID.
   4. Adoption runs are pinned to the isolated flag-on stack:
      `python3 agent_work_eval.py --manifest eval/e07_e08_adoption_cases.json run --service-protocol simple --condition service_api --expect-build-revision "$REV" --expect-feature-flag semantic_lane=off --expect-feature-flag supersession_demotion=on --expect-runtime-config supersession_demotion_weight=1.5 --concurrency 3 --timeout 360 --run-id "e07-adoption-run${N}" --out "results/2026-MM-DD-e07-adoption-raw-draw${N}.json"`;
      then
      `python3 agent_work_eval.py --manifest eval/e07_e08_adoption_cases.json measure-adoption --input "results/2026-MM-DD-e07-adoption-raw-draw${N}.json" --out "results/2026-MM-DD-e07-adoption-draw${N}.json"`.
4. Regrade disputed artifacts with the manifest before the subcommand, for
   example
   `python3 agent_work_eval.py --manifest eval/recent_work_cases.json regrade --input "$INPUT" --out "$OUTPUT"`.
5. Aggregate only the declared claim-scored artifacts:
   `E07_MAIN=(results/2026-MM-DD-e07-supersession-{flag,base,filesystem}-draw{1,2,3}.json); python3 eval/aggregate_draws.py "${E07_MAIN[@]}" --expected-arm e07-flag --expected-arm e07-base --expected-arm e07-filesystem --out results/2026-MM-DD-e07-aggregate.json`.

## Metrics

- Claims/56 per arm per draw.
- Dedup-family paired case wins/losses/ties (flag vs baseline), per draw and summed.
- Forbidden-assertion rate: rubric-flagged assertions of a superseded (stale) fact, counted separately inside and outside the family.
- Adoption rate: eligible sessions emitting valid `supersedes` frontmatter / eligible sessions.
- Context chars/case per arm (overfetch guard; legacy reference ~41,441 chars/case).

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

Embeddings (usage-billed OpenAI, explicitly exempt, listed separately): none required — all arms run exact+lexical; no semantic-ready profile exists. If semantic indexing is later added to the fixture, cost ≤ $0.19 (9.6M-token corpus rate; this corpus is a fraction of that).

**Hard ceiling: $60** all-in equivalent. A 5-draw extension of the two service arms adds 2 × 14 × 2 = 56 runs = $13.44.

## Abort criteria

- Any draw shows the flag arm asserting stale facts *outside* the family at a higher rate than baseline → stop, flag off, report before any rerun.
- Write p95 during fixture import >2× the v8 concurrent-write baseline (29.0ms) → stop; this is the 07-26 unbudgeted-bookkeeping signature.
- Any usage-billed reasoning call detected, or running total exceeds $60 → abort immediately.
- ≥2 harness failures (timeouts/errors) in a draw → invalidate that draw entirely, fix, rerun; never average a broken draw.

## Reporting

The run record must contain: git commit fingerprint; per-arm flag configuration; manifest version and hash (recent-work-v0.3); dedup-family label list; all draw artifact paths (results/2026-MM-DD-e07-supersession-{base,flag,fs}-draw<N>.json and -adoption-); per-case paired win/loss/tie table; McNemar exact p and bootstrap CI; forbidden-assertion counts in/out of family per arm; adoption rate with the eligible-session list and each emitted frontmatter block; context chars/case per arm; total cost split into subscription-equivalent and embeddings-exempt lines; explicit pass/fail against each acceptance criterion.

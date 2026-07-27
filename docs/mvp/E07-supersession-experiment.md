# E07 — Supersession Demotion and Adoption Experiment

Status: Specified — not run
Date: 2026-07-27
Gates: D04 (D04-supersession-current-truth.md)
Phase: 1 (requires flagged feature build)

## Question

Does explicit `supersedes` frontmatter with demotion-plus-annotation (D04-supersession-current-truth.md) win the dedup/current-over-history family without harming anything else — and, separately, will agents actually author the frontmatter unprompted? The second question is load-bearing: an oracle-seeded corpus proves the mechanism, not the deployed behavior.

## Preconditions and build items

1. D04 implemented behind `supersession_demotion` — **M**. Anchors: apps/api/src/simple_core.rs scoring (the derived_penalty application site), write/import frontmatter parse path, read view parameter.
2. recent-work-v0.2 manifest: add correction notes carrying real `supersedes` frontmatter to the fixture corpus; label the dedup-family case subset in eval/recent_work_cases.json (must include recent-europe-calendar-dedup; expect 3–4 labeled cases whose rubric hinges on current-over-history); re-run `python agent_work_eval.py validate --manifest eval/recent_work_cases.json` — **S**.
3. Harness flag plumbing: per-arm runtime-config toggle of `supersession_demotion` from agent_work_eval.py run setup (env var or config endpoint per D04 rollout mechanism) — **S**.
4. n≥3 paired-draw aggregator eval/aggregate_draws.py (per-case win/loss/tie, exact-binomial McNemar, case-level bootstrap CIs, stdlib only) — **S**. Known build item; does not exist yet; shared, specified in E01-paired-draw-machinery-and-baseline.md.
5. Adoption-arm harness: scripted unprompted agent-work write sessions where the authoring contract is documented in workspace docs but never stated in the task prompt; an eligibility oracle labels which sessions *should* have emitted `supersedes` (session writes a note that factually corrects an existing one) — **M**.

## Arms

1. **service_api-baseline** — simplified core, flag off, v0.2 corpus (frontmatter present in source text but mechanically inert).
2. **service_api-supersession** — identical, flag on.
3. **filesystem** — instruction-restricted read-only filesystem condition, same corpus. Natural control: the frontmatter is visible as raw text, so this arm tests whether the declaration alone (without demotion/annotation machinery) is enough.
4. **adoption (measurement arm, not claim-scored)** — 12 unprompted write sessions per draw against a live workspace; contract documented, never prompted; record eligible sessions and emitted frontmatter.

## Corpus and fixtures

recent-work-v0.2: the recent suite (12 cases / 48 claims) with correction notes added as real workspace fixtures — exact paths, full source text, sha256 recorded — each carrying `supersedes:` frontmatter authored in the Markdown itself (round-trip rule). No oracle hints in any task prompt. Dedup-family label list committed in the manifest.

## Procedure

1. Preflight: clean git tree (implementation fingerprint gate), record commit; confirm flag defaults off.
2. `python agent_work_eval.py validate --manifest eval/recent_work_cases.json` — must pass on v0.2.
3. For draw N in 1..3 (minimum; extend to 5 if the aggregate is borderline):
   1. `python agent_work_eval.py run --manifest eval/recent_work_cases.json --condition service_api --concurrency 3 --timeout 360 --run-id e07-base-draw<N> --out results/2026-MM-DD-e07-supersession-base-draw<N>.json --report results/2026-MM-DD-e07-supersession-base-draw<N>.md` (flag off).
   2. Same command with flag on, `--run-id e07-flag-draw<N>`, `--out results/2026-MM-DD-e07-supersession-flag-draw<N>.json`.
   3. `python agent_work_eval.py run --manifest eval/recent_work_cases.json --condition filesystem --concurrency 3 --timeout 360 --run-id e07-fs-draw<N> --out results/2026-MM-DD-e07-supersession-fs-draw<N>.json --report results/2026-MM-DD-e07-supersession-fs-draw<N>.md`.
   4. Adoption harness: 12 sessions, artifact `results/2026-MM-DD-e07-adoption-draw<N>.json`.
4. `python agent_work_eval.py regrade` on disputed graded answers before aggregation (rescores saved answers without regeneration).
5. Aggregate: `python eval/aggregate_draws.py results/2026-MM-DD-e07-supersession-*-draw*.json --out results/2026-MM-DD-e07-aggregate.json` over the paired base/flag draws; produce McNemar exact p and bootstrap CIs.

## Metrics

- Claims/48 per arm per draw.
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

- Claim-scored runs: 3 arms × 12 cases × 3 draws = 108 runs.
- Adoption sessions: 12 × 3 draws = 36 runs.
- Total 144 runs × $0.24 ≈ **$34.56**. Regrade passes ≈ $0 (no regeneration).

Embeddings (usage-billed OpenAI, explicitly exempt, listed separately): none required — all arms run exact+lexical; no semantic-ready profile exists. If semantic indexing is later added to the fixture, cost ≤ $0.19 (9.6M-token corpus rate; this corpus is a fraction of that).

**Hard ceiling: $60** all-in equivalent. Headroom covers reruns of invalidated draws and a 5-draw extension of the two service arms (2 × 12 × 2 = 48 runs ≈ $11.52).

## Abort criteria

- Any draw shows the flag arm asserting stale facts *outside* the family at a higher rate than baseline → stop, flag off, report before any rerun.
- Write p95 during fixture import >2× the v8 concurrent-write baseline (29.0ms) → stop; this is the 07-26 unbudgeted-bookkeeping signature.
- Any usage-billed reasoning call detected, or running total exceeds $60 → abort immediately.
- ≥2 harness failures (timeouts/errors) in a draw → invalidate that draw entirely, fix, rerun; never average a broken draw.

## Reporting

The run record must contain: git commit fingerprint; per-arm flag configuration; manifest version and hash (recent-work-v0.2); dedup-family label list; all draw artifact paths (results/2026-MM-DD-e07-supersession-{base,flag,fs}-draw<N>.json and -adoption-); per-case paired win/loss/tie table; McNemar exact p and bootstrap CI; forbidden-assertion counts in/out of family per arm; adoption rate with the eligible-session list and each emitted frontmatter block; context chars/case per arm; total cost split into subscription-equivalent and embeddings-exempt lines; explicit pass/fail against each acceptance criterion.

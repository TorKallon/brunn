# D07 — Lesson Artifacts: agent-authored procedure notes at checkpoint time

Status: Proposed — not started
Date: 2026-07-27
Depends on: D01, D02, D03, D04
Gated by: none yet — the inline experiment sketch below MUST be promoted to a numbered Exx spec and passed before any default-on
Runtime flag: lesson_artifacts

**CONDITIONAL DESIGN. Do not build until D01–D04 (docs/mvp/) have all landed. This is the closest proposal in the MVP set to the reverted derived-content failure (2026-07-22 cross-query dedup: reduced/derived context HURT quality). It therefore carries the hardest self-gates in this document set: strict score caps, trigger-only activation, role isolation, and a zero-displacement ship gate. If any gate is awkward to enforce, that is a signal to kill the design, not the gate.**

## Problem and evidence

Repeated tasks barely improve: matched repeat scored 46/64 vs 45/64 — the system re-derives procedure from scratch every run, and the chronic failing cases (ruptureops-flowworks-campaign-revision, ruptureops-forked-agent-idempotency, recent-aether-morning-brief, and peers) fail the same way across runs. The 57-case strict draw showed 21/22 disputed simplified answers already had a rubric-accepted source in returned context — losses are context *compilation*, not retrieval. A durable, agent-authored procedure note ("when reconciling archive imports, check X before Y, sources: …") targets exactly that compilation step.

The counter-evidence is equally settled: derived content injected into context is the known failure shape (dedup revert), and research indicates cross-role procedure transfer can be *negative*. Lessons must never displace primary sources and never leak across roles.

## Design

**Authoring.** At checkpoint time, the agent may author a lesson note: a plain Markdown vault file (e.g., `lessons/<role>/<slug>.md`) with frontmatter `kind: lesson`, role/agent tags, declared trigger terms, and exact source refs in the checkpoint format (`path | version N | sha256:...`). It is written through the normal write path (entries + entry_versions + workspace_changes) as an ordinary entry, *after* checkpoint commit — never inside the checkpoint transaction. Because a lesson IS vault Markdown, the Markdown-authority round-trip is native: rebuild-from-vault preserves lessons with zero extra machinery.

A lesson must cite ≥1 resolvable source ref. OWNER DECISION: whether a lesson with no resolvable refs is indexed as an ordinary note or excluded from the index entirely (recommended: excluded from lesson surfacing, retained as inert Markdown).

**Retrieval.** Lessons are indexed like any entry but surfaced only when ALL hold:

1. **Trigger match:** the query lexically matches the lesson's declared trigger terms (exact/lexical only — no semantic triggering; semantic stays off the critical path).
2. **Role match:** the requesting agent's role/agent tag matches the lesson's tags. No cross-role surfacing, per the negative-transfer research.
3. **Score cap below primary sources:** lessons carry the existing derived_penalty pattern and their final score is hard-capped below the lexical flat base (cap 2.5, below lexical's 3.0 floor), so a lesson can never outrank any primary lexical or exact hit.

**Budgets.** ≤2 lesson candidates per response; ≤1,200 excerpt chars each; counted *inside* the existing 96,000-char budget (never additive), and lessons never consume open-hydration slots ahead of primary sources. Staleness: at index time, a lesson whose source refs no longer resolve against current heads is marked stale and excluded from surfacing (a derived check, rebuildable).

## What this does NOT change

- No schema expansion: lessons are ordinary entries; frontmatter (kind, roles, triggers) is parsed at chunk-index time into derived index data only.
- Primary-source ranking is untouched; the exact/lexical/semantic score formulas do not change. Lessons only ever add below-cap candidates.
- Checkpoint contract unchanged: deterministic id, 11 rows/~55KB, parent validation; the lesson write is a separate ordinary write after commit, so the checkpoint p95 gate (baseline 17.1ms, gate 2,000ms) is unaffected by construction.
- No validity intervals, no graph database, no synchronous global consistency; dreaming stays paused (this is not dreaming — authorship happens only at explicit checkpoint time, in the agent's own turn).

## Failure-mode analysis

- **Dedup revert / derived-content displacement (the direct ancestor):** mitigated by the 2.5 score cap, trigger-only + role-only activation, in-budget accounting, and the zero-displacement ship gate below. Every context-shaping change is guilty until proven by n≥3 paired draws.
- **Displacement via budget pressure:** even below-cap candidates consume chars. The ship gate compares evidence sets directly: primary sources present with the flag off must remain present with it on, per case, every draw.
- **Cross-role negative transfer:** enforced at query time by tag match. OWNER DECISION: the initial role/agent tag taxonomy.
- **07-26 synchronous bookkeeping collapse:** lesson authoring is an ordinary post-checkpoint write; nothing new runs synchronously in checkpoint or search write paths. Soak gates re-run regardless.
- **Overfetch (~70.8K chars/case RuptureOps):** bounded at 2 × 1,200 chars inside the existing budget; chars/case is a recorded metric in the experiment.
- **Paraphrase loss / v6 recent-first:** lessons quote exact source refs and are surfaced by trigger terms, not recency or usage.

## Experiment sketch (inline; promote to an Exx before build)

- **Cases:** warmind-parser-learning plus the agent-work learning/matched-repeat cases (the 46/64 vs 45/64 set).
- **Seeding:** author a fixed set of lessons from first-run transcripts of solved cases; inject into the fixture corpus as vault files so both arms share identical corpora except surfacing.
- **Arms:** service_api with `lesson_artifacts` on vs off, identical seeds; n≥3 paired draws via agent_work_eval.py; McNemar exact binomial through the n≥3 aggregator (build item, Small, does not exist yet).
- **Ship gate:** paired claims WIN (a tie kills — added machinery must pay) AND **zero displacement**: for every case and draw, the set of rubric-accepted primary sources appearing in evidence with the flag off is a subset of the set with the flag on. Any displacement anywhere = kill.
- Also record chars/case and turns/case; single-draw deltas are noise (±3–5 claims).
- This sketch is deliberately incomplete against the experiment template. Before any build, the promoted Exx MUST add all five missing elements: (1) preflight cost arithmetic shown, (2) a hard spend ceiling, (3) abort criteria, (4) the Codex-subscription fail-closed rule with embeddings-exempt spend listed separately, and (5) exact runnable commands with artifact naming. A promoted spec lacking any of the five is not a gate and does not unlock the build.

## Acceptance gates

1. Deterministic: on a replayed query corpus, no lesson ever outranks any primary exact/lexical candidate; role-mismatched lessons never surface; stale-ref lessons never surface; per-response lesson count and char budgets asserted in tests.
2. Round-trip: rebuild-from-vault reproduces all lessons and their surfacing behavior identically (frontmatter is the only source of lesson metadata).
3. Soak: checkpoint and write p95 unchanged within noise of v8 baselines (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json).
4. Experimental: the promoted Exx passes its ship gate (paired win, zero displacement) before default-on.

## Rollout and kill switch

Runtime flag `lesson_artifacts`, default off, no-deploy toggle, with sub-keys `lesson_artifacts.author` and `lesson_artifacts.surface`. Kill = `surface` off, immediate; authored lessons remain inert Markdown in the vault — no cleanup, no migration, no authority impact. Kill criteria: any displacement of a primary source, any paired loss or tie, any checkpoint/write p95 regression, or trigger-term surfacing observed outside declared roles.

## References

- Vault notes: 2026-07-22 cross-query dedup revert; 2026-07-26 production collapse postmortem; cross-role procedure-transfer research note.
- 57-case strict draw and matched-repeat results (46/64 vs 45/64; 21/22 disputed-context finding).
- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json — checkpoint/write baselines.
- D01-budget-contracted-retrieval.md, D02-verbatim-span-contract.md, D03-resume-delta-packets.md, D04-supersession-current-truth.md — prerequisite designs; D06-wiki-link-leads.md — sibling conditional design sharing the derived_penalty and kill-switch patterns.

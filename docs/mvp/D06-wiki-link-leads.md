# D06 — Wiki-Link Leads: pointer-only link expansion in search responses

Status: Conditional — not implemented; E11 prerequisite-aborted
Date: 2026-07-27
Depends on: D01, D02
Gated by: E11
Runtime flag: link_leads

**CONDITIONAL DESIGN. Do not build or activate until D01 and D02 (docs/mvp/) have landed AND the owner corpus is imported to the simplified core with a passed fidelity audit (Tier A of the D14 authority frame). Synthetic fixtures are not link-rich; nothing about this design can be validated before real vault content is on the new core.**

## Problem and evidence

The simplified core ranks candidates by exact/lexical/semantic lanes only. It is blind to the vault's explicit link structure: `[[wiki-links]]` and relative Markdown links encode author-declared relationships between notes that no scoring lane can see. The chronic failing cases skew relational/multi-source — ruptureops-archive-import-reconciliation, ruptureops-spatial-evidence, recent-europe-calendar-dedup — and the 57-case strict draw showed 21/22 disputed simplified answers already had a rubric-accepted source in returned context: losses are context compilation, not retrieval. What agents lack is cheap *leads to the right neighbors*, not more excerpt bytes.

More bytes is actively dangerous: RuptureOps overfetch is ~70,814 service chars/case vs legacy 41,441 — the leading quality risk. Any link feature must therefore add **zero excerpt characters**.

Evidence honesty, stated up front: the project's own graph expansion experiment (vault note, graph one-hop expansion experiment) showed BM25 tied or beat gated one-hop expansion, and naive expansion cut recall 96.9%→90.6%. Content-injecting link expansion is a completed negative experiment here. This design survives only as pointers: capped, off by default, killed on noise.

## Design

**Derived link table (rebuildable projection).** At chunk reindex — the same async worker path that handles embedding backfill — parse the entry's current version for `[[wiki-link]]` targets and relative Markdown links (`[text](path.md)`). Store rows `(source_entry_id, target_path, resolved_entry_id NULLABLE, link_text)` in a derived table. This is explicitly the permitted shape under the hard constraints: a derived, rebuildable projection that never blocks reads and regenerates entirely from vault Markdown. Unresolved targets are stored with NULL `resolved_entry_id` and are never surfaced.

**Parsing is async, never in the write path.** The 2026-07-26 production collapse came from unbudgeted synchronous bookkeeping; link parsing runs strictly as a worker job after write commit. The write path does not touch the link table, ever.

**Response delta: `linked_leads`.** Search responses gain an optional `linked_leads` array. Each item is `{reference, path, title}` — pointer-only, zero excerpt chars. Cap: **≤6 linked_leads PER RESPONSE, total** — not per candidate (10 candidates × 3 each would be 30 leads of noise). Selection order: leads of the highest-scored candidates first; leads whose target is already a candidate in the response are deduplicated away. Protocol overhead is bounded: ≤~200 chars per lead → ≤1,200 chars/response, inside the existing protocol-to-evidence ratio gate (≤1.0).

**Activation.** Leads are attached only when (a) the request sets an explicit `expand_links: true` flag, or (b) the query is classified relational/multi-source. OWNER DECISION: whether (b)'s heuristic (e.g., multiple distinct entities or batched multi-query requests) is enabled at first draw, or the feature launches flag-only and the heuristic is a follow-up under the same runtime flag. A third path exists for evaluation only: runtime config `link_leads.force_attach=true` attaches leads to every search response regardless of (a)/(b) — this is the test mode E11's treatment arm uses (E11-wiki-link-leads-experiment.md, build item 7), never enabled in production.

**Budgets.** Excerpt budgets are untouched: 128 candidates, 96,000 excerpt chars/response, 2,400 chars/excerpt, ≤3 sections/entry. Lead lookup adds at most one bounded query per search request (join or single follow-up), asserted by the per-operation query-count budget.

## What this does NOT change

- No schema expansion beyond a derived rebuildable projection; no graph database; no validity intervals.
- No scoring change: exact 10.0 / lexical 3.0-base / semantic lanes are untouched. Leads are appended metadata, never scored candidates.
- Markdown authority round-trip is native: links live in the vault text; the table regenerates from it. Nothing durable is authored outside Markdown.
- Semantic stays off the critical path; parsing is purely lexical over stored text.
- Open, checkpoint, and changes contracts are unchanged.

## Failure-mode analysis

- **Naive-expansion recall collapse (96.9%→90.6%):** mitigated by pointers-only, per-response cap of 6, and zero injected content — the agent must explicitly open a lead to spend budget on it.
- **07-26 synchronous bookkeeping collapse:** parsing is worker-only; acceptance gates include a write-p95 soak with reindex churn running. Write-path latency regressed twice in one day (v5 3,404ms, v7 3,170ms unrelated-write p95) and only the 640K soak caught it — the soak gate is mandatory, not advisory.
- **Overfetch (~70.8K chars/case):** zero excerpt chars added; E11 gates chars/case to ±2%.
- **2026-07-22 dedup-revert lesson:** every context-shaping change is guilty until proven; single-draw deltas are noise (±3–5 claims). Ship decision rests solely on the n≥3 paired draw in E11.
- **v6 recent-first collapse:** leads are ordered by candidate score and link structure only — no usage/recency ranking, which is a completed negative experiment.
- **Paraphrase loss:** leads carry path/title verbatim from the vault; no generated or derived text.

## Acceptance gates

Deterministic (pre-experiment):

1. Two consecutive rebuilds of the link table from the vault produce identical row sets.
2. Replayed query corpus: leads ≤6 per response in all cases; zero excerpt chars in every lead (test assertion); unresolved links never emitted.
3. Per-operation query-count budget: search with leads enabled adds ≤1 query vs disabled.
4. `python performance_eval.py run --label d06-linkparse-soak --future-soak --out results/2026-MM-DD-d06-linkparse-soak.json` (30 samples) with link-parse reindex churn active: write p95 and concurrent write/search probe within existing gates and within noise of the v8 baseline (29.0ms concurrent write, 100.9ms concurrent search, results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json); no drift with change-log growth.

Experimental: E11 (E11-wiki-link-leads-experiment.md) passes — n≥3 paired McNemar non-inferiority on claims, lead-follow yield ≥20%, chars/case within ±2%.

## Rollout and kill switch

Runtime flag `link_leads`, default off, runtime-togglable with no deploy (hard-constraint kill switch). Two sub-toggles: `link_leads.index` (worker parsing) and `link_leads.surface` (response field). Kill = `surface` off, effective immediately; the derived table is inert and can be dropped or rebuilt at any time with no authority impact.

Kill criteria: lead-follow yield <20% in E11; chars/case regression >2%; any write-p95 soak regression; paired-claims loss. On kill, record the negative result next to the graph expansion experiment note and do not retry without a new mechanism.

## References

- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json — v8 640K soak baselines.
- v5/v7 future-soak result JSONs — write-path regression precedent.
- results/2026-07-27-3340-clean-30-sample.json — clean fixture baselines.
- Vault notes: graph one-hop expansion experiment (BM25 tie/win; recall 96.9%→90.6%); 2026-07-22 cross-query dedup revert; 2026-07-26 production collapse postmortem.
- D01-budget-contracted-retrieval.md, D02-verbatim-span-contract.md — prerequisite designs; D14-migration-and-authority-tiers.md (authority-tier frame).
- E11-wiki-link-leads-experiment.md — gating experiment.

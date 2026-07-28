# D04 — Explicit Supersession and the Current-Truth View

Status: Implemented behind flag — E07 mechanism passed; Tier C blocked
Date: 2026-07-28
Depends on: none
Gated by: E07 (E07-supersession-experiment.md)
Runtime flag: supersession_demotion

## Experiment decision

E07 completed at revision
`791fac70f7345e51d732e3965f4e770e9107d0e3`. The mechanism passed its frozen
quality, safety, deterministic, and overfetch gates:

- supersession-family paired case-instances were 6 flag wins / 2 baseline
  wins / 4 ties, net +4 against the +3 minimum;
- family claim slots were 29/48 flag versus 25/48 baseline; the
  case-clustered expected per-draw delta was +1.333 claims with 95% CI
  [-2.333, 3.667], and majority-collapsed exact McNemar p = 1.0;
- there were zero new forbidden assertions outside the family and no
  significantly negative outside-family asymmetry;
- flag/base model-visible context ratio was 1.00812659, below 1.05; and
- flag-on 64K/30 write p95 was 27.401 ms, below 58.0 ms, with the reviewed
  query-count contract green.

This accepts the mechanism under the predeclared gate, but not as a precise
effect-size estimate: the bootstrap interval includes zero and the collapsed
McNemar comparison has no discordant full-case outcomes.

The load-bearing adoption gate failed. Only 7/18 eligible unprompted
supersession sessions emitted syntactically valid frontmatter (38.889%).
Companion replay against the exact projection implementation found 6/18
resolvable distinct edges, 5/18 selected projected edges, and 3/18
noncompeting selected emissions. The gap is material:

- one syntactically accepted edge superseded its own write path and was
  filtered by the projection;
- three calendar emissions created parallel successors for a target that
  already had a canonical successor; two happened to win the lexical
  tie-break and one was shadowed; and
- no emitted target was unresolved and no actual projection cycle was
  created.

Decision: D04 remains default-off and is not Tier-C ready. Implement the
predeclared assisted-authoring path before relying on it: use
`supersession_warnings` to suggest a target when a write appears to correct an
existing note. The follow-up should also reject self-edges in adoption
measurement and surface parallel-successor ambiguity instead of counting every
syntactically valid declaration as equally useful. Requalify adoption after
those changes; do not rewrite or regrade the frozen E07 result.

Evidence:
[results/2026-07-28-e07-aggregate.md](../../results/2026-07-28-e07-aggregate.md),
[results/2026-07-28-e07-conclusion-audit.json](../../results/2026-07-28-e07-conclusion-audit.json),
and
[results/2026-07-28-e07-adoption-semantic-audit.json](../../results/2026-07-28-e07-adoption-semantic-audit.json).

## Problem and evidence

The dedup/current-over-history family is chronic across every run: recent-europe-calendar-dedup, ruptureops-archive-import-reconciliation, and ruptureops-flowworks-campaign-revision fail repeatedly. The 57-case strict draw shows the shape of the failure: 21/22 disputed simplified answers had a rubric-accepted source in the returned context — losses are context compilation, not retrieval. When an original note and its correction are both retrieved with equal standing, the model must guess which is current, and it guesses wrong at a chronic rate.

Two completed negative experiments constrain the fix:

- 2026-07-22 cross-query dedup reduced context and HURT quality — reverted. Removing candidates is not available as a mechanism.
- v6 recent-first lexical hid older authoritative sources (Star Rupture 0/3). Inferring currency from recency or usage is a completed negative experiment.

Therefore the signal must be explicitly authored, and it must demote rather than exclude.

## Design

**Authoring contract (Markdown authority).** A correction note declares supersession in its own YAML frontmatter:

```yaml
supersedes:
  - projects/europe-trip/calendar-2026-05.md
```

Paths are exact vault-relative paths in the same namespace as `entries.path`. Because the declaration lives in the source Markdown, it survives rebuild-from-vault. Per the Markdown-authority round-trip rule, any supersession state that exists only in the service is a defect while the vault is authority.

**Service derivation (write/import).** On every workspace write and import, the service parses frontmatter (bounded: first 4KB of source, existing parse path) and resolves each `supersedes` path to an entry. Edges are held in an in-process supersession map — a derived, rebuildable projection keyed by workspace generation and refreshed from `workspace_changes`. No new tables, no schema expansion; the map never blocks reads (a map miss simply means no demotion). An unresolvable path is returned as a `supersession_warnings` item on the write response; the note is still accepted.

**Retrieval demotion — never exclusion.** With the flag on, lexical and semantic lane scoring subtracts `supersession_demotion_weight` (default 1.5, runtime config) from entries that appear as a supersession target, following the existing derived_penalty pattern in simple_core.rs scoring. The exact lane keeps its flat 10.0 — an exact-path request is explicit intent and is never demoted. Superseded entries remain fully retrievable at all times.

**Annotation.** Every candidate and every open evidence section for a superseded entry carries `superseded_by: {path, head_entry}` (~120 chars, counted inside the existing 2,400-char excerpt / 96,000-char response budgets — no budget expansion). Demotion without annotation would repeat the compilation failure; the annotation is the actual fix, the demotion is ordering hygiene.

**Current-truth view.** `read` gains an optional `view: current_truth` parameter: the service walks the supersedes chain forward to its head (cycle guard; depth cap 8; on cycle, return both endpoints plus a warning) and returns the head document with the chain listed. Default read behavior is unchanged. OWNER DECISION: whether `current_truth` becomes the default read view at Tier B, or stays opt-in.

**Budgets.** Frontmatter parse is bounded per write; map refresh amortizes onto the change-feed processing already running; query-time demotion and annotation are in-process map lookups — zero additional SQL round-trips per operation. The per-operation query-count budget assertion must show no delta.

OWNER DECISION: the default demotion weight (1.5 proposed; must stay small enough that a superseded entry with a strong lexical match still outranks weak fresh matches — exclusion by the back door is the failure to avoid).

## What this does NOT change

- No new tables or columns; no schema expansion.
- No validity intervals — the chain is a plain authored edge set with no time semantics.
- No graph database — chain walk is a bounded in-process traversal, cap 8.
- No restored synchronous global consistency — map staleness window equals the change-feed refresh interval; a write in one request may not demote in a concurrently running search.
- Candidate caps, excerpt budgets, lane structure, and the 128/96K/2,400 limits are untouched.
- Nothing is ever excluded from retrieval. Markdown remains authority. Dreaming stays paused.

## Failure-mode analysis

- **Dedup revert (2026-07-22):** that experiment removed context. D04 removes nothing — both notes stay retrievable; net context chars are unchanged apart from annotations. Every context reduction is guilty until proven; D04 performs none.
- **v6 recent-first collapse:** demotion derives only from explicit authored declarations. An old note nobody has superseded is never demoted, so older authoritative sources cannot be hidden the way v6 hid Star Rupture.
- **2026-07-26 bookkeeping collapse:** derivation is a bounded parse at write plus async map refresh — no unbudgeted synchronous work. The performance gate asserts write p95 and query counts (write-path regressed twice in one day before; only the 640K soak caught it).
- **Overfetch (~70,814 chars/case RuptureOps):** annotations add ≤~120 chars each inside existing budgets; candidate counts unchanged.
- **Paraphrase loss:** annotations are structured fields; source text stays byte-exact.
- **New risk — wrong or stale supersedes edges:** a bad edge demotes a live note. Mitigations: demotion not exclusion (the note still surfaces, annotated); edges are authored Markdown, so auditable and diffable in the vault; kill switch restores exact baseline scoring instantly.
- **Adoption risk:** the mechanism is inert if agents never author the frontmatter. E07 measures unprompted adoption explicitly; the oracle-seeded result does not transfer without it.

## Acceptance gates

Deterministic:
1. Round-trip: export vault, rebuild from Markdown, supersession map identical.
2. Cycle fixture returns a warning, never hangs; depth cap enforced.
3. Unresolvable-path fixture yields `supersession_warnings`, note accepted.
4. Per-operation query-count budget: zero delta with flag on.
5. performance_eval.py at 64k, 30 samples, flag on vs off: the gated comparison is the on-vs-off delta at 64k (within noise), plus no violation of existing hard gates. The v8 640K-soak figures (open 59.7ms, search 53.1ms, concurrent write 29.0ms — results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json) are cited as hard-gate context only, not as 64k comparison points.

Experiment: E07 (E07-supersession-experiment.md) must pass — dedup-family net ≥+3 cases across n≥3 paired draws with McNemar support, zero new forbidden assertions elsewhere, and the adoption criterion.

## Rollout and kill switch

`supersession_demotion` is a runtime config flag — no deploy needed to disable. Off: no demotion, no annotations, `view: current_truth` ignored with a notice; responses byte-identical to baseline. Sequence: ship dark → E07 → enable on Nyx during the Tier B window (D14 authority tiers) → owner data. Any ranking or lineage incident: flag off, immediate revert to measured v8 behavior.

## References

- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json — latency baselines.
- 57-case strict draw record — 21/22 disputed-context finding; chronic case list.
- 2026-07-22 dedup revert and v6 recent-first collapse (Star Rupture 0/3) — vault notes; both constrain this design.
- E07-supersession-experiment.md — gating experiment.
- D05-intention-ledger.md — shares the bounded frontmatter derivation path.

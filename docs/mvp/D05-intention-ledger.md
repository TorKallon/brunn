# D05 — Intention Ledger: Agent-Authored Prospective Memory

Status: Proposed — not started
Date: 2026-07-27
Depends on: none
Gated by: E08 (E08-intention-ledger-experiment.md)
Runtime flag: intention_ledger

## Problem and evidence

The prospective chronic family fails in every run: recent-aether-gmail-actions (0–2/4), coord-deadline-readiness (2/4), recent-aether-morning-brief. These cases require acting on a *future obligation* recorded in a past session. Nothing in the current task's queries mentions the obligation, so no retrieval lane surfaces it — retrieval answers the question asked, not the question that should have been asked. Unlike the compilation losses (21/22 disputed answers had an accepted source in context), prospective misses are frequently true retrieval absences: the obligation note never enters context at all.

Files do not fix this. A vault surfaces an intention only if the agent happens to open the right note. This design targets a capability that is files-impossible: an intention written by agent A in one client surfaces at agent B's next open in a different client. Given the interface result (files 194/228 vs native API 186/228), the right move is to build where the service can do what files cannot, rather than re-fighting ground where files win.

## Design

**Authoring contract (Markdown authority).** An intention is an ordinary vault note with frontmatter:

```yaml
kind: intention
trigger: [gmail, aether, oauth-scopes]
due: 2026-08-03
status: pending
```

Fully vault-round-tripped: rebuild-from-vault reconstructs the ledger exactly. Intentions are agent-authored only. This is explicitly NOT dreaming — no service-generated intentions, no background synthesis, Open question 8 untouched.

**Derivation.** The same bounded frontmatter parse as D04-supersession-current-truth.md (first 4KB at write/import) identifies `kind: intention`. Pending intentions (`status: pending`) live in a small in-process derived projection (expected total ≤ a few hundred notes), rebuildable, refreshed from `workspace_changes`, never blocking reads. No new tables.

**Surfacing at open.** Open already runs exact and lexical lanes over the request queries. With the flag on, the pending set is matched in-process against the open queries: trigger terms ∩ query terms under the existing FTS normalization (case-folding, stemming). The response gains:

```
pending_intentions: [ {path, title_line (≤80 chars), due, status_note, matched_terms} ] × ≤5
```

Pointer-only, ~500 chars total, hard cap enforced by a budget assertion. No source text is inlined — if the intention matters, the agent reads the note exactly via `memory.read` (paraphrase-loss guard). Matching is a lookup over the in-process pending set: zero additional SQL round-trips, zero embedding calls — semantic stays entirely off this path and off the Tier B critical path.

**Cross-agent property.** Because the projection refreshes from the change feed, an intention captured by one agent appears in every client's next open that hits its trigger terms — MCP clients, native API, all of them — with no client-side coordination.

**Completion.** The agent edits the note to `status: done` (or deletes it) via a normal write; the change feeds the projection and every other client stops seeing it. Overdue intentions (due date past, still pending) are annotated `overdue`, never silently dropped.

OWNER DECISION: whether a due-window rule (surface anything due within 7 days regardless of trigger match) is enabled at launch. Default proposed: off — trigger-match only, because untriggered surfacing raises the false-surfacing floor E08 gates on.

OWNER DECISION: eviction order when >5 intentions match. Default proposed: nearest due date first, then most recent write.

## What this does NOT change

- No schema expansion; intentions are ordinary entries, the pending set is an in-process rebuildable projection.
- No validity intervals — `due` is authored data the agent interprets; the service applies no time semantics beyond the overdue annotation.
- Search scoring is untouched; intention notes rank normally in ordinary search as well.
- Open candidate caps (≤32), hydration limits, and the 96K excerpt budget are unchanged; `pending_intentions` is a separate ≤500-char section.
- Markdown remains authority; lossless export includes intentions as plain notes.
- Semantic lane untouched. Dreaming stays paused (Open question 8).

## Failure-mode analysis

- **Overfetch (RuptureOps ~70,814 chars/case):** +≤500 pointer chars per open cannot meaningfully add bloat; the budget assertion makes violation a test failure, not a drift.
- **Dedup-revert lesson:** D05 adds context rather than removing it, but additions can still distract. E08 gates false surfacing (<10%) and runs a no-regression check on non-prospective cases in the same draws.
- **v6 recent-first collapse:** surfacing keys on explicitly authored trigger terms, not inferred usage or recency — the completed negative experiment is not repeated.
- **2026-07-26 bookkeeping collapse:** write-path cost is only the bounded frontmatter parse already required by D04; matching at open is in-process over a small set. Gates: zero added SQL queries per open, open p95 delta <10ms at 64k.
- **Paraphrase loss:** pointers only; the source note is read byte-exact on demand.
- **New risk — stale intention spam:** intentions never marked done accumulate. Mitigations: cap of 5, eviction order, overdue annotation making staleness visible, and the fact that cleanup is a normal Markdown edit.
- **Adoption risk:** inert if agents never author intentions. E08 inherits the shared adoption measurement (E07-supersession-experiment.md, arm 4 instrument).

## Acceptance gates

Deterministic:
1. Round-trip: rebuild-from-vault reproduces the pending projection exactly.
2. Open response char delta ≤500; assertion in the harness.
3. Zero additional SQL queries per open (query-count budget assertion).
4. performance_eval.py at 64k, 30 samples: open p95 delta (flag on − off) <10ms (baselines: 59.7ms at 640K soak, results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json; 26.278ms on the clean 3,340 fixture, results/2026-07-27-3340-clean-30-sample.json).
5. Flag off: open responses byte-identical to baseline.
6. `status: done` fixture never surfaces; overdue fixture surfaces with `overdue` annotation.

Experiment: E08 (E08-intention-ledger-experiment.md) must pass — prospective claim gains under n≥3 paired draws, false-surfacing <10%, no regression on non-prospective cases, latency gate.

## Rollout and kill switch

`intention_ledger` is a runtime config flag — no deploy to disable. Off: no response section, no matching work, byte-identical baseline. Sequence: ship dark → E08 → enable on Nyx in the Tier B window (D14 authority tiers) → owner data. Kill switch is instant; because the projection is derived and read-only, disabling leaves no state to clean up.

## References

- Chronic-case record: recent-aether-gmail-actions 0–2/4, coord-deadline-readiness 2/4, recent-aether-morning-brief — 57-case strict draw and repeat runs.
- Interface run on simplified core: native API 186/228 vs files 194/228.
- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json; results/2026-07-27-3340-clean-30-sample.json — open latency baselines for the <10ms gate.
- D04-supersession-current-truth.md — shared bounded frontmatter derivation.
- E08-intention-ledger-experiment.md — gating experiment.
- Decisions.md (vault) — Open question 8: dreaming paused; this design leaves it untouched.

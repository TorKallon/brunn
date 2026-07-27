# D03 — Resume Delta Packets

Status: Implemented behind a default-off flag — E06 not run
Date: 2026-07-27
Depends on: none
Gated by: E06 (E06-resume-delta-experiment.md)
Runtime flag: resume_deltas

## Problem and evidence

Transitions are the worst suite in the program: 0/5 cases in every run, in every condition. Failures are claim-slot omissions, never lineage loss — the agent resumes with correct lineage but omits claims about what changed while it was away. straylight-api-gate-transition is the worst chronic case.

The current resume payload explains why. On open with a resume_checkpoint_ref the agent receives: checkpoint text truncated to token_budget*4 chars, plus changes_since_checkpoint (≤200 rows) — a list that says which paths changed, but not what changed. Reconstructing the delta requires follow-up reads pinned to two versions, which the agent must think to issue and usually does not. The omission is structural, not a retrieval failure.

The store already holds everything needed: checkpoints carry exact source refs ("path | version N | sha256:..."), entry_versions is immutable, and workspace_changes gives generation identity. Both sides of every delta are retrievable by construction. A plain file tree categorically cannot do this — no version history, no generation log — which makes this a capability files lack, not parity with them.

Performance headroom: resume p95 is 35.2ms at the 640K soak (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json), with no latency drift as the change log grows.

## Design

With resume_deltas on, open with resume_checkpoint_ref additionally computes:

1. Intersect the checkpoint's source_refs paths with the paths in changes_since_checkpoint (already computed, ≤200 rows). Only refs in the intersection get deltas.
2. For each such ref, up to ≤8 sources: fetch the checkpoint-pinned version N (immutable entry_versions) and the entry's current_version via head. Fetching is one batched query over (entry_id, version) pairs — the per-operation budget assertion is exactly +1 round trip on the resume-open path.
3. Materialize a delta per source:
   - Small files — both versions ≤2,400 chars each — are returned whole, both versions (`mode: whole_pair`, fields before/after). Rationale: an out-of-context bare diff recreates the section-selection loss D02 targets; for small sources the full pair is cheaper and strictly more legible.
   - Larger files get a standard unified diff, 3 context lines (`mode: unified_diff`).
4. Budgets: aggregate delta budget ≤6,000 chars, charged against the open evidence budget — deltas displace other open evidence rather than growing the payload. Unified diffs have a 2,000-char per-source cap. A `whole_pair` is indivisible and may exceed that soft cap when both complete versions still fit the aggregate budget; otherwise it degrades to a pointer rather than returning a partial “whole” pair. Sources beyond the ≤8 limit or the char budget degrade to pointer `evidence_leads` annotated "changed since checkpoint: version N → M" (existing lead mechanism).
5. Prioritization when more than 8 refs changed: checkpoint source_refs order (authoring order reflects the checkpoint author's priority). The E06 build adopts this specified default; it does not introduce a recency heuristic.
6. Response delta: new open field `resume_deltas: [{path, pinned_version, pinned_sha256, current_version, current_sha256, mode, before?, after?, diff?}]`. Hashes reuse the checkpoint source-ref format, so every delta is verifiable against lineage. Request delta: none.
7. Integrity: if a pinned version's stored sha256 does not match the recomputed hash, the open fails loud with a lineage error — never a silent empty delta.

Latency gate: resume p95 ≤150ms at 640K — ~4x the measured 35.2ms v8 baseline. The loose 500ms figure is explicitly rejected; the hard gates (open ≤5,000ms) are 50-100x looser than measured and gate nothing at this granularity.

## What this does NOT change

- Write and checkpoint path: untouched. Checkpoints stay 11 rows/~55KB at any scale; no new checkpoint content is authored; deltas are computed at resume-read time only.
- No schema expansion: entry_versions, workspace_changes, and checkpoint rows are read as-is.
- changes_since_checkpoint contract (≤200) and all open caps (≤32 candidates, ≤8 hydrated docs, MAX_OPEN_COMPLETE_SOURCE_CHARS 24,000) except the internal reallocation described above.
- Markdown authority: no new durable metadata; deltas are derived projections. After rebuild-from-vault, version history restarts and the intersection is empty — the feature degrades to exactly today's behavior, correct by construction.
- Non-resume opens and search: byte-identical with flag on or off.

## Failure-mode analysis

- Dedup revert (2026-07-22): charging deltas against the evidence budget is a context reallocation, and every context reduction is guilty until proven. This is precisely why E06 is n≥3 paired with a filesystem control — the displaced evidence could matter more than the deltas.
- v6 recent-first collapse: no ranking or recency heuristic anywhere; deltas key strictly off checkpoint source_refs, the author's declared authorities.
- 2026-07-26 bookkeeping collapse: all added work is read-time, bounded (≤8 sources, +1 batched query), inside the resume-open path only, and pinned by the 150ms soak gate plus the query-count assertion. Nothing synchronous touches the write path.
- Overfetch (~70,814 RuptureOps chars/case): budget-neutral by construction; E06 records open payload chars per resume to verify neutrality empirically.
- Paraphrase/section-selection loss: whole_pair mode for small sources exists specifically to avoid handing the model a context-free diff hunk.

## Acceptance gates

1. E06: first-ever transitions case win (>0/5) under the flag, paired improvement over both service_api_resume-current and filesystem_rebuild across n≥3 draws, exact McNemar.
2. Resume p95 ≤150ms at the 640k soak (performance_eval --future-soak, 30 samples definitive) with the flag on; concurrent write/search probe unchanged vs 29.0ms/100.9ms baselines beyond noise.
3. Query-count assertion: resume open is exactly +1 round trip; non-resume paths +0.
4. Checkpoint footprint gate unchanged (harness gate ≤100 rows/4MiB; actual stays 11 rows/~55KB); protocol-to-evidence ratio ≤1.0 holds.
5. Integrity test: hash-mismatch and missing-pinned-version paths fail loud.

## Rollout and kill switch

Flag resume_deltas, default off. Sequence: eval environment for E06 → Nyx under the Tier B read/write plan (D14 frame; checkpoint-resume canaries) → default on after gates. Kill switch is the runtime flag — flip disables delta computation entirely and restores today's resume payload with no deploy. Any checkpoint-lineage incident during rollout follows the Tier C tripwire: immediate flag-off and revert to Markdown authority.

## Implementation record

- Runtime configuration: `STRAYLIGHT_RESUME_DELTAS`, default `false`, exposed through Compose as `resume_deltas`.
- Resume behavior: only a flagged open with `resume_checkpoint_ref` enters the delta path. Non-resume opens and all flag-off requests retain the previous response shape.
- History read: one batched SQL statement accepts the author-ordered `(entry_id, pinned_version)` pairs and returns pinned/current text plus hashes. Missing entries or versions, path drift, checkpoint/stored hash disagreement, and recomputed text-hash disagreement return `checkpoint_lineage_error`.
- Evidence accounting: returned before/after or diff characters are deducted from the existing evidence token allowance before ordinary hydration.
- Evaluation checkpoints: the simple evaluation importer now records exact `entry_ref`, path, version, and hash structures, including when a batched import placed a source in an earlier batch.
- Agent projection: both `native_memory.py` and the MCP reasoning view preserve `resume_deltas` and annotated delta pointers.
- Verification is unit and harness-level only in this commit. The definitive 640K/30-sample performance and query-count gates remain part of E06/D09 execution, not an implementation result.

## References

- E06-resume-delta-experiment.md — gating experiment.
- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json — resume p95 35.2ms, concurrent 29.0/100.9ms at 640K.
- Transitions evidence: 0/5 in every run; claim-slot omissions, never lineage loss; straylight-api-gate-transition worst.
- apps/api/src/simple_core.rs — open/resume path, changes_since_checkpoint, checkpoint source refs; migrations 0051-0055.
- D02-verbatim-span-contract.md — sibling design in the exact-value loss family.
- Documented negative results: 2026-07-22 dedup revert; v6 recent-first collapse; 2026-07-26 bookkeeping collapse.

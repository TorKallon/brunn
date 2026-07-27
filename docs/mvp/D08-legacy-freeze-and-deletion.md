# D08 — Legacy Freeze and Deletion

Status: Proposed — not started
Date: 2026-07-27
Depends on: D14 (authority tiers; D14-migration-and-authority-tiers.md)
Gated by: none for the freeze and the immediate cleanups below; full legacy deletion is gated by launch gates 2 and 4 (fidelity import and restore proof, per D14's master gate list) plus an n≥3 paired-draw parity experiment (experiment ID unassigned — a NEW Exx is genuinely required: E01 and E10 compare the simplified core against filesystem conditions, not against legacy. OWNER DECISION: assign when the parity spec is written)
Runtime flag: n/a — the freeze is a compile-time cargo feature (`legacy-core`), not a context-shaping change

## Problem and evidence

The legacy core is the largest single mass of unowned risk in the repo: `read_service.rs` (~10.0K lines), `write_service.rs` (~6.3K lines), the legacy worker and dreams pipeline, and the legacy `/v1/memory/*` routes. The product decision (vault, 2026-07-27) makes the simplified core (`apps/api/src/simple_core.rs`, migrations 0051–0055) the go-forward path, with the Markdown vault as authority and fallback until launch gates pass.

Two facts prevent deleting legacy today:

1. **Parity is close but not proven.** The 57-case strict draw scored legacy 170/228 vs simplified 160/228 (direct Markdown 171/228), and the interface run on the simplified core scored native API 186/228 vs files 194/228. The single-draw noise floor is ±3–5 claims (agent-work native swung 40→47→44→43→47 across builds), so none of these deltas is load-bearing on its own. Byte-perfect export into a core that loses reasoning quality is not migration safety.
2. **Export tooling lives in legacy.** Premature deletion strands the owner corpus: hosted straylight.rourkem.com is still legacy at migration 50, and Nyx holds the simplified schema with empty tables. No deployment holds owner data on the new core.

Meanwhile the legacy dream pipeline actively burns cycles: the dream-retry loop re-attempts an over-budget bundle (the 755,784-byte case) every 15 minutes, forever. The 2026-07-26 production collapse came from exactly this class of unbudgeted background bookkeeping.

## Design

**Freeze now.** Move all legacy-core modules (read_service, write_service, legacy worker/dreams, `/v1/memory/*` route registration) behind a cargo feature `legacy-core`. Default builds exclude it; the hosted legacy deployment and the export tooling build with the feature enabled. Frozen means: no functional changes accepted into legacy code except the dream-retry fix below and security patches; CI compiles both feature states.

**Delete now (test-covered, nothing calls them):** the MCP reasoning-view legacy residue — `memory.compute`/`verify` entries and the `hydrated_sources` branches in apps/mcp. These are dead on the 12-tool `/v1/workspace/*` surface and their removal is covered by existing tests.

**Fix now: dream-retry loop.** Mark over-budget dream bundles as permanent failures so the 15-minute retry stops. This changes no behavior anyone depends on — dreaming stays paused (Open question 8). The fix ships inside the frozen boundary because it removes load, not because dreaming is resuming; D14 (D14-migration-and-authority-tiers.md) lists it as a Tier B precondition.

**Delete later, only when both hold:**
- (a) Launch gates 2 and 4 pass (per D14's master gate list): gate 2, owner-corpus export→import with a full fidelity audit — paths/bytes/sha256 identical, binary descriptions byte-copied (never regenerated), every `parent_checkpoint_id` resolves — and gate 4, the restore drill with the fidelity audit re-run, zero diffs.
- (b) An n≥3 paired-draw parity result with exact-binomial McNemar shows the simplified core is not significantly behind legacy on the agent-work suites. Requires the n≥3 aggregator (build item, Small; eval/aggregate_draws.py per E01-paired-draw-machinery-and-baseline.md). Note this comparison does not exist elsewhere: E01/E10 pair simplified against filesystem arms, never against legacy, so the legacy-parity Exx must be written and run before deletion.

**SPA:** freeze the SPA to a read-only ops console — status, manifest, usage, binaries. The legacy capture and learned-context surfaces are dropped at deletion time, not before, so the ops console keeps working against the legacy deployment through the migration window.

**Legacy usage telemetry (migration 0022): do not port.** `entry_usage` plus the append-only `workspace_changes` feed suffice. The v6 recent-first collapse (Star Rupture 0/3) closed usage/recency ranking as a completed negative experiment, so this telemetry has no future consumer.

## What this does NOT change

- No schema change on the simplified core; no new tables, no validity intervals, no graph structures.
- Markdown vault remains authority and fallback; the freeze does not advance any authority tier by itself.
- The `/v1/workspace/*` contract and the 12 MCP stdio tools are unchanged (the residue deletions remove dead code only).
- Dreaming remains paused; the retry fix is a stop-the-bleeding change, not a re-enable.
- The hosted legacy deployment keeps running unmodified (feature-enabled build) until Tier C cutover per D14 (D14-migration-and-authority-tiers.md).

## Failure-mode analysis

- **Stranded corpus (the defining risk):** deleting legacy before export→import passes the fidelity audit destroys the only tested export path. Mitigated by making gate (a) a hard precondition and tagging a `legacy-final` release before any deletion lands.
- **Silent quality loss (dedup-revert / paraphrase-loss pattern):** the 2026-07-22 cross-query dedup experiment reduced context and hurt quality; 21/22 disputed simplified answers had rubric-accepted sources in context, meaning losses are context compilation, not retrieval. A byte-fidelity audit cannot see this class of loss — hence gate (b), n≥3 with McNemar, not a single draw.
- **07-26 bookkeeping class:** the dream-retry loop is the same unbudgeted-background-work failure shape. Fixing it inside the freeze reduces the chance the legacy deployment degrades during the migration window.
- **Feature-flag rot:** a compile-time flag that is never built goes stale. CI must build and test both feature states until deletion.

## Acceptance gates

Deterministic, for the freeze:
- Default build contains no legacy symbols (compile check / symbol grep); CI green with `legacy-core` on and off.
- MCP residue deletion: existing test suite green; grep confirms no `memory.compute`, `verify`, or `hydrated_sources` references remain in apps/mcp.
- Dream-retry fix: a test injects an over-budget bundle (≥ the 755,784-byte case) and asserts it is marked permanent-failure and not re-enqueued after the 15-minute window.

For deletion (both required):
- Fidelity audit reports from launch gates 2 and 4 (import audit plus restore re-audit), zero divergence, archived alongside the export artifacts.
- n≥3 paired-draw parity run: per-case win/loss/tie, exact-binomial McNemar non-significant against the simplified core, case-level bootstrap CIs reported.

## Rollout and kill switch

Freeze rollout: land the feature flag, flip default off, verify hosted deployment builds with the feature on. Kill switch: re-enable the feature in the default build — no data migration involved. Deletion rollout: tag `legacy-final`, delete behind a single PR, keep the tag as the recovery point. Deletion is intentionally hard to reverse; that is why both gates precede it.

## References

- Vault: product decision 2026-07-27; Decisions.md (cost rules); Open question 8 (dreaming paused).
- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json (v8 baselines).
- 57-case strict draw and interface-run results (reasoning evidence set, 2026-07-27).
- D14-migration-and-authority-tiers.md (Tier A/B/C preconditions; master gate list); migration 0022 (legacy usage telemetry).

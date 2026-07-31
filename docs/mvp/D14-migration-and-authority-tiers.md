# D14 — Migration and Authority Tiers

Status: In progress — local exact-composite preflight passed; service gate pending
Date: 2026-07-27
Depends on: D13 (D13-client-integration-and-canaries.md)
Gated by: E01 (n≥3 paired-draw aggregator machinery; E01-paired-draw-machinery-and-baseline.md), E04 (result-budget experiment; E04-result-budget-experiment.md), and E10 (combined preflight, the final pre-launch gate; E10-combined-preflight.md) — Tier C only; Tiers A and B are gated by deterministic checks in this doc
Runtime flag: n/a (process document; feature flags live in the Dxx docs this plan sequences)

## Problem and evidence

The Markdown vault remains authority and fallback until launch gates pass. The simplified core is fast — v8 640K-record soak p95s of open 59.7ms, search 53.1ms, checkpoint 17.1ms, resume 35.2ms with no latency drift (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json) — but speed is not authority. Reasoning parity is within noise, not proven better (57-case strict draw: legacy 170/228, simplified 160/228, direct Markdown 171/228; single-draw swings are ±3-5 claims). Write-path latency regressed twice in one day (v5 3,404ms and v7 3,170ms unrelated-write p95, per the v5/v7 future-soak JSONs in results/), and only the 640K soak caught it. The 07-26 production collapse came from unbudgeted synchronous bookkeeping no test gated. Authority therefore moves in tiers, each with deterministic gates, and this document is the master gating record.

## Design

### Six launch gates (current status)

1. Release pinning — tag v8 at commit c3a5420 and retain the image digests. Status: PARTIAL; the release candidate exists, tag-plus-digest retention must be completed and recorded.
2. Fidelity import — owner corpus exported from legacy and imported to Nyx simplified with a zero-diff fidelity audit (spec below). Status: PARTIAL; the exact current+history+delta composite passed its local zero-diff audit, including 710 byte-copied binary-description pairs and 5,079 native-record materializations. The isolated simplified import, service audit, and downloaded-byte round trip remain pending; see [Tier-A-legacy-fidelity-runbook.md](Tier-A-legacy-fidelity-runbook.md).
3. Client canaries — all three clients pass the READ set (Tier A) and later the WRITE set (Tier B) per D13 (D13-client-integration-and-canaries.md). Status: not started.
4. Restore proof — restore drill with fidelity re-audit (Tier B), then Railway cutover PITR drill (Tier C). Status: not started.
5. Parity evidence — E01 machinery built and used for n≥3 paired draws; E04 (the result-budget experiment, E04-result-budget-experiment.md) passed; writable-sidecar comparisons run two-sided; E10 combined preflight passed (all shipped flags on under one global budget, E10-combined-preflight.md — the final pre-launch gate; no cutover proceeds without it). Status: not started (E01 aggregator and writable-sidecar control are known build items, Small/Medium).
6. Shadow verdict — shadow period completed with zero tripwires fired and an explicit owner go decision. Status: not started.

### Tier A — read-only pilot (~2-3 focused days)

Day 1: complete gate 1 (tag v8 c3a5420, retain digests). Export the legacy owner corpus from hosted straylight.rourkem.com (legacy at migration 50) with history=true so version lineage is in the manifest. Import to Nyx simplified.
Day 2: run the fidelity audit (below). Mint read-only tokens, one credential per client, via straylight_auth.admin_issue_credential per the D13 runbook. Begin the three-client READ canaries, each including the known-answer check — sufficiency != no_evidence alone is vacuous.
Day 3: finish canaries and spend the buffer on canary failures, which the record says to expect on first contact. Read-only enforcement verified (capability-derived server-side, auth.rs:125-132) — the pilot cannot corrupt the workspace.

Fidelity audit spec (zero diffs = pass; any diff = fail, no judgment calls):
- Every entry path, byte length, and sha256 identical versus the legacy export.
- Binary descriptions BYTE-COPIED, never regenerated — regeneration is the extraction-hallucination vector.
- Every parent_checkpoint_id resolves.
- Counts match the manifest, including version lineage (history=true).

The read-only current-snapshot bridge and its machine-readable scoped audit are
documented in [Tier-A-owner-snapshot-tooling.md](Tier-A-owner-snapshot-tooling.md).
That bridge can prove exact supported-text import for a disposable evaluation
user and select E11 candidates, but it explicitly cannot satisfy this full gate:
binary publication, `history=true` lineage, and checkpoint-table parent
resolution remain required.

The full legacy recovery/replay implementation and honest current gate record
are documented in
[Tier-A-legacy-fidelity-runbook.md](Tier-A-legacy-fidelity-runbook.md). Its
aggregate preflight result is
`results/2026-07-27-tier-a-legacy-fidelity-preflight.json`. The local composite
has zero differences across 4,926 paths and 4,955 versions. D14 gate 2 now
passes: all six stages imported into a fresh isolated simplified workspace,
the service audit matched all legacy versions and 5,079 native
materializations, the checkpoint resumed, and the full-history byte audit
matched 10,009 current paths plus 10,038 historical versions with zero
differences. The owner capture's single checkpoint has no non-null parent
reference, so parent-resolution evidence for this capture remains honestly
vacuous. Release pinning and the D13 READ canaries are still required for full
Tier A.

### Tier B — read/write daily driver (+3-4 days)

Entry requires Tier A complete. Contents:
- Restore drill: snapshot Railway PG + S3 (or Nyx volumes), restore to scratch, re-run the fidelity audit, require zero diffs.
- WRITE canaries per D13 (stale-version conflict, idempotency no_op, checkpoint/resume round-trip, gapless changes cursor, advisory-lock 409).
- Concurrent probe at owner scale, watching unrelated-write p95 specifically — it regressed twice in one day and only the 640K soak caught it.
- Dream-retry fix shipped (dreaming itself stays paused, Open question 8).
- ~~Crude open/search char budget near the legacy ~41.4K chars/case at entry~~ **Amended 2026-07-28:** the char-budget entry condition is withdrawn — E01's paired baseline falsified its overfetch premise (service 32,067 chars/case vs files 101,406; RuptureOps 63,090 vs 142,640, CI entirely negative). Replacement entry condition: D09 regression-tier gates green on the deployed build AND the E12 loss-autopsy triage disposition recorded (proceed / proceed-with-mitigations / hold). The MD fallback still protects against data loss, not quietly worse daily work — E12 is now the guard for the latter.
- Semantic indexing stays OFF the critical path (the query-embedding call at simple_core.rs:3005 is synchronous and uncached; no semantic-ready latency profile exists).

### Tier C — sole authority (+5-8 build days + 2-4 weeks calendar)

Entry requires (amended 2026-07-28): E01 machinery built and its baseline complete (done — non-inferiority was not established at n=3 and the −5-claim margin was underpowered against the observed CI width); the parity requirement is satisfied either by a future owner-approved n≥3 synthetic result or by shadow-period real-work evidence, at the owner's explicit Tier C entry decision; E04/E05/E06 resolved as drops (done); E10 combined preflight passed on the frozen launch manifest (E10-combined-preflight.md); writable-sidecar comparisons run two-sided (done in E01); Railway simplified cutover done; PITR drill passed.

Shadow protocol with pre-defined abort tripwires (defined before the window opens, not during):
- Any checkpoint-lineage incident = immediate abort to MD authority. No triage-first.
- Weekly lossless-export diff versus the MD vault must be zero-divergence.
- Latency SLOs held on real traffic (current hard gates: open p95 ≤5,000ms, search ≤3,000ms, read ≤1,000ms, checkpoint ≤2,000ms — 50-100x looser than measured baselines).
- One live restore executed mid-period.
- Per-release soak gate for anything merged during the shadow window — no unbudgeted bookkeeping mid-shadow; that is the 07-26 lesson.
- The period ends only with an explicit owner go decision. OWNER DECISION: shadow window length within the 2-4 week range, and the go/no-go call itself.

### Cost preflight (required for ALL tiers, not just Tier C)

Per Decisions.md: all reasoning runs via the ChatGPT-authenticated Codex subscription, fail-closed (require_codex_subscription rejects API keys); usage-billed OpenAI only for embeddings (exempt; ~$0.19 per 9.6M-token corpus); observed all-in ≈$0.24/agent-run equivalent (470-run audit, $113.18).
- Tier A: three READ canary sets plus failure retries ≈ 30-60 agent-run equivalents ≤ $15 all-in; embeddings-exempt spend $0 (semantic pending is expected).
- Tier B: WRITE canaries, concurrent probe, one definitive 30-sample soak ≈ 60-100 equivalents ≤ $25 all-in; one optional corpus indexing ≈ $0.19 embeddings-exempt.
- Tier C: reasoning spend is specified in the gating experiments' own preflights, not re-derived here — E01: 513 runs ≈ $123.12 worst case (498 ≈ $119.52 on the default transitions-deferral path), ceiling $150; E10: 342 runs ≈ $82.08 (+optional draw 4 ≈ $27.36, worst case ≈ $109.44), ceiling $120 — plus targeted 5-draw chronic repeats carried inside each Exx's own ceiling. Each Exx spec carries its own preflight arithmetic, hard ceiling, and abort criteria — Tier C does not start without them.

## What this does NOT change

Markdown-authority round-trip is preserved through Tier B: any new durable metadata must be authored/representable in Markdown so it survives rebuild-from-vault. No schema expansion, no validity intervals, no graph database, no restored synchronous global consistency. Semantic stays off the Tier B critical path. Every context-shaping change sequenced by this plan still ships behind its own runtime flag and its own n≥3 experiment.

## Failure-mode analysis

- Plausible-but-wrong canaries (07-10): known-answer checks mandatory in every canary (D13).
- Unbudgeted bookkeeping (07-26): per-release soak gate during shadow; round-trip/query-count budget assertions accompany latency gates.
- Quietly worse daily work (dedup revert; v6 recent-first collapse, Star Rupture 0/3): E12 loss-autopsy triage at Tier B entry (replacing the withdrawn char budget) and two-sided comparisons at Tier C; every context reduction is guilty until proven.
- Regenerated binary descriptions (paraphrase/extraction hallucination): byte-copy required by the fidelity audit.
- Single-draw overconfidence: only E01-aggregated n≥3 draws are load-bearing for Tier C.

## Acceptance gates

Tier A: gates 1-2 pass; gate 3 READ set passes for all three clients. Tier B: gate 4 restore drill zero-diff; gate 3 WRITE set; concurrent probe within hard gates; D09 regression tier green; E12 triage disposition recorded (2026-07-28 amendment). Tier C: gates 5-6; zero tripwires; owner go recorded.

## Rollout and kill switch

The kill switch is the tier structure itself: at any tier, abort returns authority to the Markdown vault, which remains lossless-exportable throughout. A checkpoint-lineage incident aborts immediately without owner consultation; all other aborts are owner calls against the tripwire list.

## References

- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json; results/2026-07-27-3340-clean-30-sample.json; v5/v7 future-soak JSONs in results/.
- D13-client-integration-and-canaries.md; E01-paired-draw-machinery-and-baseline.md (aggregator machinery); E04-result-budget-experiment.md (result-budget experiment); E10-combined-preflight.md (final combined pre-launch gate).
- Tier-A-owner-snapshot-tooling.md (read-only current-snapshot preflight, scoped audit, and honest gate boundary).
- Tier-A-legacy-fidelity-runbook.md and results/2026-07-27-tier-a-legacy-fidelity-preflight.json (full-history recovery, exact binary companion contract, native-record materialization, replay, and current partial gate record).
- Decisions.md (cost rules); Operations.md (credential storage); vault notes on the 07-10, 07-22 dedup, 07-26, and v6 recent-first incidents.

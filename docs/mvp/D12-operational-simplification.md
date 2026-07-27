# D12 — Operational Simplification

Status: Proposed — not started
Date: 2026-07-27
Depends on: D08 (legacy freeze; D08-legacy-freeze-and-deletion.md), D14 (authority tiers; D14-migration-and-authority-tiers.md)
Gated by: none (infrastructure; the Railway cutover itself is a Tier C event gated by D14's tripwires)
Runtime flag: n/a for infrastructure items; `embedding_backfill_guard` for the backfill throttle

## Problem and evidence

Operational surface is currently wider than the product: two object-store paths (MinIO everywhere, S3 nowhere), two candidate production hosts (hosted straylight.rourkem.com on legacy at migration 50; Nyx on the simplified schema with empty tables), and an unbounded monitoring wishlist. Specific evidence:

- The MinIO image in deployable artifacts carries a 3-critical/26-high CVE finding — a standing release blocker that patching-and-tracking would turn into a permanent tax.
- The 2026-07-26 production collapse came from unbudgeted synchronous bookkeeping that no test gated; v5/v6 soaks showed embedding index catchup contending with foreground traffic. Background work must be structurally isolated, not just polite.
- Write-path p95 regressed twice in one day (v5 3,404ms, v7 3,170ms unrelated-write p95, per the v5/v7 future-soak JSONs) and only the 640K soak caught it — monitoring must watch the few metrics that have actually caught real regressions.
- All latency baselines (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json) are exact+lexical with embeddings pending; the initial owner-corpus embed is a future bulk operation with no production precedent.

## Design

**S3-only production object store.** Production uses AWS S3 exclusively: versioned bucket, SSE enabled. MinIO is pinned to the dev/test compose file only and never appears in a deployable image — this retires the CVE blocker by removal rather than remediation. Binary verification (sha256) is store-agnostic and unchanged. OWNER DECISION: bucket region and whether versioning retention gets a lifecycle policy or stays indefinite until first cost review.

**Single hosted target after cutover: Railway.** One production: static SPA (read-only ops console per D08), private API service, worker service, PostgreSQL 17 with pinned pgvector and PITR. Nyx is demoted to restore-rehearsal and backup target — it exists to prove restores work (Tier B restore drill, Tier C PITR drill per D14-migration-and-authority-tiers.md), never as a second production accepting writes. MCP stays a local stdio process on the client machine (apps/mcp, env-var auth, `STRAYLIGHT_API_URL` pointed at the hosted API) — it is not a hosted component and gains nothing from being one.

**Worker stays a separate service.** v5/v6 showed index catchup contending with foreground reads. The worker (embedding backfill, change-feed projections, any future async accelerators) runs as its own Railway service with its own resources. Semantic indexing stays off the Tier B critical path; the API must serve exact+lexical fully with the worker stopped.

**Datadog, trimmed to what has caught real problems:**
- Metrics: per-route p95s, queue depth and oldest-job age, PG health, disk.
- Exactly four monitors: API down; write p95; queue age; backup success.
- Queue-age monitoring is polling-shaped — a jitter in poll cadence looks like queue stall. Thresholds must tolerate the documented false-alarm mode: alert on sustained breach (multiple consecutive windows), not single samples. A monitor that cries wolf weekly is worse than no monitor.

**Embedding backfill rate limit with a foreground-latency guard.** The initial owner-corpus embed (~$0.19 per 9.6M-token corpus; usage-billed OpenAI, explicitly exempt from the subscription rule) runs through the worker at a configured rate limit. The guard: worker samples foreground open/search p95 and pauses backfill while p95 exceeds a configured multiple of the v8 baseline (default: pause above 2x open 59.7ms / search 53.1ms sustained). Controlled by `embedding_backfill_guard` so backfill can be halted at runtime without a deploy — the canonical flag name, shared with D11-semantic-lane-policy.md section (d); one guard, one name in config.

## What this does NOT change

- No schema change; no new tables; the simplified core contract (`/v1/workspace/*`, 12 MCP tools) is untouched.
- Markdown vault remains authority; Railway cutover happens only at Tier C with D14's shadow period and abort tripwires (checkpoint-lineage incident → immediate abort to MD authority; weekly lossless-export diff must show zero divergence; per-release soak gate during shadow).
- Semantic search remains an optional async accelerator, never a gate — worker isolation enforces this structurally.
- Credential model unchanged: minted once via `straylight_auth.admin_issue_credential`, unrecoverable; read-only capability-derived server-side (auth.rs:125-132).
- No new monitoring vendor, no APM expansion, no log-pipeline project.

## Failure-mode analysis

- **07-26 bookkeeping collapse:** the direct motivation for hard worker separation and for the backfill guard — background work that can degrade foreground must be pausable at runtime and isolated by service boundary.
- **v5/v7 write regressions caught only by the 640K soak:** monitors alone are insufficient; D14's per-release soak gate during shadow remains mandatory. The write-p95 monitor is the production echo of that gate, not a replacement.
- **v5/v6 index-catchup contention:** the reason the worker is a separate service and the guard samples foreground p95 rather than trusting the rate limit alone.
- **MinIO CVE blocker:** solved by absence; the residual risk is dev/test-vs-prod drift, mitigated by running the restore drill and binary-verification checks against real S3, not MinIO.
- **Queue-age false alarms:** documented polling-shaped failure mode; sustained-breach thresholds prevent alert fatigue from burying the one real page.

## Acceptance gates

1. Deployable image scan: no MinIO binary or image layer in any production artifact; CVE scan clean at critical/high for object-store components.
2. Restore drill: back up from production S3+PG, restore onto Nyx, re-run the fidelity audit (paths/bytes/sha256, binary descriptions byte-copied, parent_checkpoint_id resolution) — zero divergence. PITR drill to an arbitrary point succeeds before Tier C.
3. Worker isolation probe: with the worker stopped, full exact+lexical service; performance_eval correctness markers green and p95s within v8 baselines.
4. Monitor verification: each of the four monitors fired by a synthetic fault (API stopped; injected slow write; stalled queue job; failed backup) exactly once, with queue-age proven quiet across a normal week.
5. Backfill guard test: bulk embed against a 64k-scale fixture with concurrent foreground load; guard pauses and resumes; foreground open/search p95 never exceeds the configured multiple.

## Rollout and kill switch

Order: (1) S3-only images and dev/test MinIO pinning; (2) worker split and backfill guard on Nyx; (3) Datadog trim; (4) Railway environment stood up and soaked in shadow per D14; (5) cutover at Tier C. Kill switches: `embedding_backfill_guard` halts backfill at runtime; DNS-level rollback to the legacy hosted deployment remains available until D08's deletion gates pass; Nyx restore path is rehearsed, not theoretical.

## References

- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json (baselines; concurrent probe); v5/v7 future-soak JSONs (write regressions, catchup contention).
- Vault: 2026-07-26 production collapse post-mortem; Decisions.md (subscription fail-closed, embeddings exempt, ~$0.19/corpus).
- D08-legacy-freeze-and-deletion.md (SPA freeze, legacy deployment lifecycle); D14-migration-and-authority-tiers.md (tiers, shadow tripwires, drills); D11-semantic-lane-policy.md (shared embedding_backfill_guard).

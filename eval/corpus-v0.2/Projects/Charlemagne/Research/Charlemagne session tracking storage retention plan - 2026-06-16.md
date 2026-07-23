Created: 2026-06-16
Updated: 2026-06-16
Status: Active handoff summary
Related: [[Projects/Charlemagne/Charlemagne|Charlemagne]], [[Projects/Warmind/Warmind|Warmind]], [[Projects/Charlemagne/Logbook|Logbook]], [[Projects/Charlemagne/Research/Charlemagne session tracking backend implementation - 2026-06-04|Session tracking backend implementation]], [[Projects/Charlemagne/Research/Charlemagne session tracking production diagnostic - 2026-06-08|Session tracking production diagnostic]], [[Projects/Warmind/Warmind rhea storage cleanup todo - 2026-05-27|Warmind rhea storage cleanup todo]]

Execution source of truth: `/Users/rourkem/projects/warmind/docs/specs/session-tracking-storage-retention-plan.md`

## Purpose

Keep Logbook session tracking enabled while stopping the new session-tracking tables from forcing daily rhea EBS expansion.

The feature was just announced and people like it. The plan is therefore not to turn it off. The plan is to keep durable session product data, shorten retention for pipeline/idempotency state safely, and compact raw working data after a session is safely published and replay-safe.

## Current production evidence

Read-only rhea triage on 2026-06-16 found:

- `/` at `3.2T` total, `2.9T` used, `298G` free, `91%`.
- `charlemagne` MySQL at `159.76 GiB`, up from an older May baseline near `53.99 GiB`.
- `session_activity_events` at `58.43 GiB`, about `30.5M` rows.
- `play_sessions` at `43.30 GiB`, about `3.8M` rows.
- `session_pgcr_ingest` at `3.57 GiB`, about `18.6M` rows.
- `session_achievement_state` at `0.31 GiB`.
- A 30 second sample showed `session_activity_events` adding `3,456` rows and `play_sessions` adding `87` rows.
- Recent sessions were mostly complete: `9,925` published, `73` open, `2` suppressed, and `1` finalizing in a 10,001-session sample.
- Recent ingest rows were mostly terminal: `40,616` processed, `1,979` ignored, `33` processing, and `6` queued in the sampled range.

## Adversarial review changes

Five adversarial review passes pushed the plan from "cleanup schedule" to "gated production rollout." The repo spec now requires:

- runway and free-space gates before bulk mutation;
- indexed dry-run shapes and production `EXPLAIN` before cleanup;
- idempotency-preserving tombstones or equivalent before deleting ingest rows;
- replay/rehydration proof before nulling `session_activity_events.statsJson`;
- read-path replacement before event-row pruning;
- explicit participant privacy/deletion handling;
- no assumption that logical deletes or null updates return EBS space;
- physical reclaim as a separate approved window.

## Data decision

- Keep `play_sessions` as durable product state.
- Keep session summaries and achievements, but version and compact them before old-row backfill.
- Treat `session_pgcr_ingest` as an idempotency/recovery ledger, not disposable scratch. Clean it only after tombstones or horizon fences preserve duplicate/replay safety.
- Treat `session_activity_events.statsJson` as hot working data for sessionization, summaries, and achievements. It can be nulled or archived only after durable summaries, replay proof, and refinalization behavior are in place.
- Add a skinny long-term `session_instance_map` projection so PGCR-to-session links and participant lookups do not depend on full raw event rows.

## One-week gate sequence

1. Baseline and stop signs: capture fresh `df`, table sizes, row growth, queue/recovery state, stale statuses, and live config; calculate runway and decide whether temporary EBS is needed before bulk work.
2. Observability and dry-run tooling: add low-load metrics, indexed dry-runs, cleanup pause reasons, and disabled-by-default cleanup jobs.
3. Ingest retention: add the needed index, tombstone/skip ledger, duplicate-job fences, and chunked cleanup only for terminal rows older than both TTL and recovery/backfill horizon.
4. Read-path replacement: add/backfill/dual-write `session_instance_map`, define deterministic PGCR fallback behavior, and make session detail work summary-only.
5. Raw stats compaction: add markers, replay/rehydration proof, stale-status repair, conservative grace windows, and small batches.
6. Summary compact storage: enable for new writes only after measured compression ratios and dual-read/dual-write rollback support.
7. Physical reclaim decision: schedule separately with snapshot, free-space budget, DDL matrix, and rollback plan if logical cleanup does not return enough filesystem space.

## Guardrails

- Do not turn off the feature as the primary fix.
- Do not mutate production data without a current read-only baseline and explicit maintenance window.
- Keep cleanup interruptible, chunked, and disabled or dry-run by default.
- Do not prune raw stats until late refinalization can rehydrate from durable PGCR bytes or is explicitly blocked.
- Do not delete event rows until PGCR lookup, participant lookup, and owner/private detail paths have summary/map-backed replacements.
- Do not run physical reclaim operations without a lock, runtime, free-space, and rollback plan.

## Open decisions

- Raw stats retention window: 7, 14, or 30 days.
- Processed/ignored ingest retention window: 7 or 14 days, plus tombstone strategy.
- Whether raw stats need cold archive storage.
- Participant snapshot privacy policy.
- Compression format for summaries.
- Partitioning versus chunked cleanup plus later rebuild.
- Whether physical reclaim happens this week or after logical growth is under control.

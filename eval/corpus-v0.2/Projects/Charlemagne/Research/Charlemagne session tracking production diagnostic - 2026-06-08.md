# Charlemagne session tracking production diagnostic - 2026-06-08

Created: 2026-06-08
Status: read-only production diagnostic snapshot
Related: [[Projects/Charlemagne/Charlemagne|Charlemagne]], [[Projects/Warmind/Warmind|Warmind]], [[Projects/Charlemagne/Research/Charlemagne session tracking Nyx handoff - 2026-06-08|Session tracking Nyx handoff]]

## Scope

This note captures the local diagnostic findings from the 2026-06-07 PDT / 2026-06-08 UTC production incident where SweeperGo queue length spiked and session tracking was suspected.

Incident window covered here: 2026-06-08 06:19-06:56 UTC / 2026-06-07 23:19-23:56 PDT.

The diagnostic pass used Datadog API/log/metric reads and local source inspection only. No production DB, Redis, S3, process, queue, or config changes were made by the diagnostic agent. Rourke independently disabled sessions and restarted `warmind-cortex` plus `warmind-parser` during the investigation window.

Treat this as a timestamped evidence packet, not current production truth. Any Nyx/Rhea follow-up should recheck live state before making a root-cause or cleanup claim.

## Executive read

Session tracking was a real contributor to the incident, but it was not the only queue in the screenshot.

Evidence supporting session tracking involvement:

- `features.session-tracking` appeared enabled in Datadog health metrics during the active window.
- Parser logged repeated warnings:

```text
session tracking parser eligibility lookup failed; enqueueing for cortex retry: context deadline exceeded
```

- `charlemagne.session_tracking.parser_enqueue` showed about `44,938` `queued`, `44,951` `not_charlemagne_user`, and `18` `redis_error` outcomes over the 90 minute diagnostic window.
- `charlemagne.sweepergo.qlen{task:session_pgcr_seen}` built to roughly `1.5k-1.8k`, then dropped to `0` after sessions were disabled and parser/cortex restarted.
- `charlemagne.session_tracking.sessions_open` went from zero-emitted health to rapidly growing open-session gauges, with `pve` over `4.3k` and one-PGCR sessions over `6.4k` by the end of the active session-tracking window.
- `sessions_finalized_per_min` capped around `10/min`, matching the default finalize batch and suggesting finalization throughput was much lower than observed open-session growth.

Evidence that the screenshot mixed multiple queues:

- `sync_clan_members` also spiked around 2026-06-08 06:19-06:24 UTC, peaking around `12k-14k` depending on scalar versus binned query.
- Cortex logs showed thousands of `CLAN_MEMBER_SYNC: Finished Clan Sync...` messages during that earlier burst.
- The large stacked SweeperGo chart should not be read as purely session work unless grouped by `task`.

Containment looked effective in immediate post-disable bins:

- `session_tracking.enabled` reported `0` in post-restart bins around 2026-06-08 06:53:30Z and 06:54:30Z.
- `session_pgcr_seen` queue length dropped to `0` after 2026-06-08 06:53:54Z.
- Parser enqueue metrics dropped to `0` in later bins.
- The last observed parser session timeout warning was 2026-06-08 06:52:22 UTC.

## Timeline

| Time UTC | Time PDT | Observation |
|---|---|---|
| 2026-06-08 06:19-06:24 | 2026-06-07 23:19-23:24 | `sync_clan_members` queue spike. Scalar max was about `14,115`; binned max about `12,743`. Cortex logs showed thousands of clan sync completions. |
| 2026-06-08 06:24-06:27 | 23:24-23:27 | Session health metrics begin emitting as enabled. Open sessions were zero before this emitted window. |
| 2026-06-08 06:27 onward | 23:27 onward | `sessions_open` grows steadily across mode families. `session_pgcr_seen` queue begins building later in the window. |
| 2026-06-08 06:40-06:52 | 23:40-23:52 | Parser logs 18 session tracking eligibility timeout warnings. |
| 2026-06-08 06:53:35 | 23:53:35 | Cortex logs `RECEIVED SIGNAL: terminated`. |
| 2026-06-08 06:53:50 | 23:53:50 | Cortex logs `STARTUP: warmind-cortex v0.10 starting up... (prod)`. |
| 2026-06-08 06:54:01 | 23:54:01 | Parser logs `STARTUP: warmind-parser v1.0d build 003 starting up... (prod)`. |
| 2026-06-08 06:54:58 | 23:54:58 | Parser startup gap check logs `total gaps found: 108, queued PGCRs: 128`. |
| 2026-06-08 06:53:54 onward | 23:53:54 onward | `session_pgcr_seen` queue length reports `0`; parser session enqueue metrics report `0` in later bins. |

## Datadog evidence

Dashboard and logs:

- Parser and Cortex dashboard: [Datadog dashboard](https://app.datadoghq.com/dashboard/fcb-5bm-dcw)
- Session timeout logs: [Datadog logs query](https://app.datadoghq.com/logs?from_ts=1780896282439&live=false&query=service%3Awarmind-parser+%22session+tracking+parser+eligibility+lookup+failed%22&stream_sort=desc&to_ts=1780901682439)

Top log-volume pass, last 45 minutes of active investigation:

| Service | Status | Count |
|---|---:|---:|
| `warmind-parser` | info | 74,458 |
| `warmind-cortex` | info | 45,020 |
| `warmind-parser` | warn | 9,178 |
| `warmind-api` | warn | 2,270 |
| `warmind-cortex` | warn | 1,403 |
| `warmind-parser` | error | 175 |
| `warmind-cortex` | error | 103 |
| `warmind-api` | error | 11 |

Session-related log search over the diagnostic window:

| Pattern | Count | Service | Status |
|---|---:|---|---|
| `session tracking parser eligibility lookup failed; enqueueing for cortex retry: context deadline exceeded` | 18 | `warmind-parser` | warn |
| `REGISTRATION: Session abort...` / `callbackDiscordAuth... session state <nil>` | 8 total | `warmind-api` | warn/info |

The API registration/OAuth session logs were small-volume user-auth noise. They do not explain the SweeperGo queue spike.

SweeperGo queue ranking:

| Queue task | Observed max |
|---|---:|
| `sync_clan_members` | about `12k-14k`, depending on scalar versus binned query |
| `session_pgcr_seen` | about `1.5k-1.8k`, depending on scalar versus binned query |
| `update_charl_stats` | about `208` |
| `wishlist_alerts` | about `33-37` |
| `session_finalize_due` | `0` |
| `session_recover_pgcr_seen` | `0` |

Session tracking metrics:

| Metric | Observation |
|---|---|
| `charlemagne.session_tracking.enabled` | `1` during active session-tracking window, then `0` in post-disable bins around 06:53:30Z and 06:54:30Z. |
| `charlemagne.session_tracking.parser_enqueue` | About `44,938 queued`, `44,951 not_charlemagne_user`, `18 redis_error` over 90 minutes. Dropped to zero after disable/restart. |
| `charlemagne.session_tracking.sessions_finalized_per_min` | Capped around `10/min`, consistent with batch size `10` and one `session_finalize_due` job per minute. |
| `charlemagne.session_tracking.sessions_open{mode_family:pve}` | Rose to about `4.3k`. |
| `charlemagne.session_tracking.sessions_open{mode_family:other}` | Rose to about `1.9k`. |
| `charlemagne.session_tracking.sessions_open{mode_family:pvp}` | Rose to about `1.19k`. |
| `charlemagne.session_tracking.sessions_open{mode_family:dungeon}` | Rose to about `623`. |
| `charlemagne.session_tracking.sessions_open{mode_family:mixed}` | Rose to about `601`. |
| `charlemagne.session_tracking.sessions_open{mode_family:raid}` | Rose to about `536`. |
| `charlemagne.session_tracking.sessions_open{mode_family:gambit}` | Rose to about `36`. |
| `charlemagne.session_tracking.sessions_open_by_pgcr_count{pgcr_count_bucket:1}` | Rose to about `6.4k`, so most open sessions had only one PGCR. |
| `charlemagne.session_tracking.sessions_open_inactive` | Mostly zero except `inactive_bucket:180_360m`, which rose to about `457`. |

Immediate post-restart/disable checks:

- Cortex restart visible at 2026-06-08 06:53:50 UTC.
- Parser restart visible at 2026-06-08 06:54:01 UTC.
- `session_pgcr_seen` queue:
  - 06:51:30Z: about `704`
  - 06:52:30Z: about `1,364`
  - 06:53:30Z: about `937`
  - 06:53:54Z and later bins: `0`
- `parser_enqueue`:
  - nonzero `queued` and `not_charlemagne_user` before 06:53:54Z
  - zero in later bins

## Source anchors

Local source paths inspected in `/Users/rourkem/projects/warmind`:

- `sweeperbot/sessiontracking.go`
- `sweeperbot/sweeperbot.go`
- `dev/config/warmind_config.toml`
- `charlemagne/migrations/20260604_0001_session_tracking.sql`
- `docs/specs/session-tracking-backend-plan.md`

Important behavior found in local source:

- `EnqueueSessionTrackingPGCRSeenIfCharlemagneUser()` returns immediately when `features.session-tracking` is false.
- It builds a PGCR pointer, extracts membership IDs, then calls `sessionTrackingLookupUsers()`.
- `sessionTrackingLookupUsers()` uses Redis hash `charlie:all_user_memid_map` with a `250ms` timeout.
- If Redis lookup times out, parser logs the warning above.
- On lookup error, code increments `session_tracking.parser_enqueue{status:redis_error}` and deliberately continues with `eligible = nil`.
- Because `err != nil`, the "no eligible Charlemagne users" fast return is bypassed and the PGCR is enqueued with `EnqueueUniqueByKey(session_pgcr_seen, instanceId)`.
- This is fail-open behavior: Redis eligibility lookup failure converts otherwise-filterable PGCRs into cortex retry work.

Worker setup from local source:

- `session_pgcr_seen` is registered at medium priority with `MaxFails: 8` and `MaxConcurrency: sessionTrackingPGCRConcurrency()`.
- Default `warmind-cortex.session-tracking-pgcr-concurrency` is `2`.
- `session_finalize_due` is registered at medium priority with `MaxConcurrency: 1`.
- If session tracking is enabled, `session_finalize_due` is enqueued every minute.
- Default `warmind-cortex.session-tracking-finalize-batch` is `10`.

Persistent tables from migrations:

- `charlemagne.session_pgcr_ingest`
- `charlemagne.session_activity_events`
- `charlemagne.play_sessions`
- `charlemagne.session_achievement_state`
- `charlemagne.session_commends`
- `charlemagne.session_comments`

## Working hypothesis

The best-supported shape is:

1. Session tracking was enabled in production.
2. Parser began high-rate session tracking enqueue work.
3. Parser-side eligibility filtering depended on Redis `charlie:all_user_memid_map` with a tight `250ms` timeout.
4. Redis lookup timeouts caused fail-open enqueue behavior.
5. Even without many Redis errors, normal eligible PGCR enqueue volume was high enough to create thousands of open sessions and a `session_pgcr_seen` backlog.
6. Cortex finalization throughput was capped around `10/min`, far below observed open-session growth.
7. A separate hourly `sync_clan_members` burst inflated the total SweeperGo queue chart, making task-level isolation necessary.
8. Disabling sessions and restarting stopped session queue growth immediately.

This is not yet a code root-cause claim. It is a diagnostic read from Datadog plus local source inspection. Nyx should verify deployed source/config/state before deciding on a fix.

## Open questions

- Was `features.session-tracking` intentionally enabled in production before the incident?
- Did `charlie:all_user_memid_map` have stale, missing, or slow data at the incident edge?
- Were the high `not_charlemagne_user` counts expected, or does parser need a cheaper pre-filter before Redis lookup?
- Should parser fail closed on Redis eligibility timeout instead of fail open?
- Should `session_pgcr_seen` enqueue be limited by parser-side rate, queue depth, or a circuit breaker?
- Is `session_finalize_due` throughput intentionally capped at `10/min`, and is that enough for launch/backfill traffic?
- Are the open `play_sessions` rows from this incident harmless, or do they need a read-only inventory and later approved cleanup?
- Did the parser startup gap recovery after restart queue only expected modern gaps, or did it introduce a new parser/cache concern?

## Recommended Nyx next steps

Keep this read-only unless Rourke explicitly approves a write or service action.

1. Verify current production state from Datadog:
   - `avg:charlemagne.session_tracking.enabled{host:rhea}`
   - `max:charlemagne.sweepergo.qlen{host:rhea,task:session_pgcr_seen}`
   - `sum:charlemagne.session_tracking.parser_enqueue{host:rhea} by {status}.as_count()`
   - `avg:charlemagne.session_tracking.sessions_open{host:rhea} by {mode_family}`
   - `sum:charlemagne.error{host:rhea,sev:error} by {method}.as_rate()`
2. Verify current production config read-only:
   - confirm `features.session-tracking = false`
   - confirm `warmind-cortex.session-tracking-*` knobs
3. Verify parser/cortex process state and deployed code revision read-only.
4. Query DB read-only for incident-created session state:
   - counts by state in `charlemagne.play_sessions`
   - counts by `createdAt`, `updatedAt`, `startedAt`, `expiresAt`
   - counts by `activityCount` and `primaryModeFamily`
   - ingest status counts in `charlemagne.session_pgcr_ingest`
   - recent rows in `charlemagne.session_activity_events`
5. Check whether open sessions stopped growing after disable/restart.
6. Only after the read-only inventory, discuss code or data cleanup with Rourke.

## Stop rules

Stop and discuss with Rourke before:

- changing production config
- restarting/stopping services
- writing to DB, Redis, S3, queues, or production filesystem state
- clearing/draining queues
- running data cleanup
- committing a code change based only on the local diagnostic snapshot


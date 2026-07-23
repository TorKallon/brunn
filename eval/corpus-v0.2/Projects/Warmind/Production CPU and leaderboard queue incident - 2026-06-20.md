Created: 2026-06-20
Status: incident note
Repo: /Users/Shared/projects/warmind
Related: [[Projects/Warmind/Warmind|Warmind]], [[Projects/Warmind/D1 parser|D1 parser]]

## Summary

On 2026-06-20, production `rhea` CPU pressure was driven primarily by MySQL and Cortex DB work, not by the parser process itself.

Datadog current half-hour snapshot:
- CPU idle around `1.5%`; user CPU around `88%`.
- Process CPU: `mysqld` around `2855%`, `warmind-cortex` around `965%`, `warmind-parser` around `108%`, `warmind-api` around `164%`.
- Normal parser throughput around `85 PGCR/s`, close to yesterday's same-window `84 PGCR/s`.
- Parser is still in `opportunistic` error-reparse mode, not the prior `scaled` failure mode.

Same-window comparison against 2026-06-19:
- Yesterday had about `48%` idle CPU, zero queue for the checked Cortex queues, and parser tx duration around `89 ms`.
- Current parser tx duration was around `318 ms`, with parser DB queue waits averaging about `2044 ms` and peaking above `23 s`.
- `mysqld` process CPU increased from about `674%` to about `2855%`.

## Queue Evidence

The largest current queue is `global_profile_leaderboards`:
- Current queue around `622k`.
- Current starts/dones around `11-12/s`.
- Current average job duration around `14 s`, p95 around `32 s`.
- Yesterday same window: queue `0`, starts/dones around `1.1/s`, average duration around `6 s`.

Session tracking is also showing DB contention:
- `session_pgcr_seen` queue around `116k`; rate around `35/s`, similar to yesterday, but average duration increased from about `40 ms` yesterday to about `2.36 s` now.
- `session_finalize_due` is not a huge queue, but average duration increased from about `1.9 s` yesterday to about `99 s` now.
- Current Cortex DB error patterns are mostly `session_tracking.session_finalize_due... Error 1213: Deadlock found when trying to get lock`.

Other current queues:
- `update_clan_info` around `89k`, current job duration around `2.9 s` vs about `0.6 s` yesterday.
- `sync_clan_members` around `10k`, current duration around `22 s`.
- `update_charl_stats` queue small, but duration increased from about `17 ms` to about `508 ms`.

## Interpretation

This does not look like an overnight code deploy or service restart. Datadog `systemd.unit.uptime` had parser/API/Cortex at about 25 hours uptime, with no reset inside the last 24 hours.

The likely causal story is:
1. A delayed wave of `global_profile_leaderboards` work became active.
2. That job is registered with `MaxConcurrency: 0` in `sweeperbot/sweeperbot.go`, so it is effectively unbounded in gocraft/work terms.
3. The recent profile leaderboard ops command uses `global_profile_leaderboards` and schedules with `12h + deterministic_hash(membershipId) % 48h`, so work can arrive later even when code/config did not change overnight.
4. The active leaderboard wave and session fanout are making each DB-backed unit of work much more expensive.
5. Matching parser rate is therefore misleading: the unit cost rose sharply, and MySQL became the limiting resource.

The original live enqueue transcript/count was not found in session search, so treat the delayed-wave link as strongly supported by current queue shape plus known scheduling behavior, not as proven from command output.

## First Mitigation To Consider

Pause `global_profile_leaderboards` first, then remeasure MySQL CPU, parser DB wait, `session_pgcr_seen`, and `session_finalize_due`.

The low-blast-radius gocraft/work pause key shape is:

```text
sweeperbot:jobs:global_profile_leaderboards:paused
```

This should pause new fetches and let in-flight jobs finish. If DB pressure remains high after that drain period, the next candidates are session tracking pressure, especially `session_pgcr_seen` and `session_finalize_due`.

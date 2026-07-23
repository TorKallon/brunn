# Charlemagne rhea storage bleed triage - 2026-06-24

Created: 2026-06-24 08:05 PDT
Updated: 2026-06-24 08:05 PDT
Status: live read-only triage

## Related
- [[Projects/Charlemagne/Charlemagne|Charlemagne]]
- [[Projects/Warmind/Warmind|Warmind]]
- [[Projects/Charlemagne/Research/Charlemagne rhea storage and MySQL tuning findings - 2026-05-22|Charlemagne rhea storage and MySQL tuning findings - 2026-05-22]]
- [[Projects/Charlemagne/Research/Charlemagne session tracking storage retention plan - 2026-06-16|Charlemagne session tracking storage retention plan - 2026-06-16]]
- [[Projects/Warmind/Warmind rhea storage cleanup todo - 2026-05-27|Warmind rhea storage cleanup todo - 2026-05-27]]

## Scope
Read-only production storage attribution on `rhea.warmind.io` after concern that root storage was still dropping quickly.

No production data was changed. No services or long-running jobs were stopped. The local Warmind checkout was dirty and behind `origin/master`, so repo code was used only for orientation; live host/MySQL state is the truth for this note.

## Headline
At the end of this pass, `/` had recovered to about `496G` free (`3.7T` total, `3.2T` used, `87%`). The current durable MySQL growth story is `charlemagne` session/cutover storage, especially retained `_legacy` tables. The current transient "bleed then rebound" story is plausibly MySQL temporary work from a running Season 30 WLR/SRL repair audit over `nexus.skill_rating_match_audits`.

## Current root disk evidence

Direct SSH checks:

| Time PDT | `/` total | Used | Avail | Use |
|---|---:|---:|---:|---:|
| 2026-06-24 07:52 | `3.7T` | `3.2T` | `486G` | `87%` |
| 2026-06-24 07:57 | `3.7T` | `3.2T` | `496G` | `87%` |
| 2026-06-24 08:05 | `3.7T` | `3.2T` | `496G` | `87%` |

Datadog `system.disk.free{host:rhea,device:/dev/nvme0n1p1}` over 14d showed:

- steady decline from about `570.8e9` bytes on 2026-06-10 to a pre-grow low near `177.5e9` bytes on 2026-06-21;
- a jump after the root volume grow on 2026-06-21;
- another apparent decline in the hourly bins through 2026-06-23/24;
- raw 1m samples on 2026-06-24 showed free space around `3.12e11` to `3.19e11` bytes, then a sudden jump to about `5.31e11` bytes, matching the later direct `df`.

Interpretation: there is a real growth trend, but the most recent sharp drop/recovery looks transient, not a durable file/table growth event.

## MySQL schema totals

Snapshot time: 2026-06-24 14:55 UTC.

| Schema | Total GiB | Data GiB | Index GiB | Data free GiB | Rows estimate |
|---|---:|---:|---:|---:|---:|
| `nexus` | `1732.74` | `1272.55` | `460.18` | `29.81` | `13687159628` |
| `charlemagne` | `273.95` | `237.04` | `36.91` | `0.74` | `1071523691` |
| `nexustoo` | `8.46` | `5.54` | `2.92` | `0.00` | `81302789` |
| `discord` | `6.39` | `4.99` | `1.39` | `0.08` | `28185170` |

Baseline comparison:

- 2026-05-22 note: `charlemagne` was about `53.99 GiB`; `nexus` was about `1612.11 GiB`.
- 2026-06-16 note: `charlemagne` was about `159.76 GiB`.
- 2026-06-24 live: `charlemagne` is about `273.95 GiB`.

So `charlemagne` added about `114.2 GiB` since 2026-06-16. `nexus` added about `120.6 GiB` since 2026-05-22, but over a longer window and with some older candidates now reduced.

## Largest current MySQL tables

| Table | Total GiB | Data GiB | Index GiB | Rows estimate | Notes |
|---|---:|---:|---:|---:|---|
| `nexus.summary_atime` | `181.88` | `181.88` | `0.00` | `934444665` | Still huge, roughly same as May. |
| `nexus.raw_raids` | `178.71` | `84.82` | `93.89` | `714727707` | Still huge, roughly same as May. |
| `nexus.raw_dungeons` | `146.97` | `70.49` | `76.48` | `535187445` | Still huge. |
| `charlemagne.session_activity_events_legacy` | `102.61` | `90.94` | `11.67` | `51299065` | Large retained legacy session table. |
| `nexus.summary_strikes_detailed` | `93.21` | `93.21` | `0.00` | `1378522354` | Large summary family. |
| `nexus.summary_gambit` | `89.96` | `71.09` | `18.87` | `325400169` | Large summary family. |
| `nexus.skill_rating_match_audits` | `81.99` | `78.48` | `3.51` | `15660916` | Also involved in active repair duplicate audit. |
| `charlemagne.play_sessions_legacy` | `74.29` | `72.93` | `1.36` | `7540363` | Large retained legacy session table. |
| `nexus.summary_weapon_meta_daily` | `69.88` | `51.21` | `18.67` | `938030401` | Much lower than May's `140.84 GiB`; prior reclaim likely happened. |
| `nexus.skill_rating_player_deltas` | `58.00` | `41.93` | `16.07` | `190527195` | Large skill-rating storage. |

## Session/cutover table inventory

The migration history lines up with the `_legacy` table timestamps:

- `20260619_0001_session_tracking_normalized_storage` applied at `2026-06-21 05:17:56 UTC`.
- `20260622_0001_session_tracking_lean_cutover` applied at `2026-06-22 19:07:34 UTC`.

Largest session-family tables:

| Table | Total GiB | Rows estimate | Latest observed row by monotonic key |
|---|---:|---:|---|
| `session_activity_events_legacy` | `102.61` | `51299065` | `2026-06-22 19:05:18 UTC` |
| `play_sessions_legacy` | `74.29` | `7540363` | `2026-06-22 19:05:46 UTC` |
| `session_instance_lookup_hot` | `8.63` | `23333138` | not sampled |
| `session_instance_lookup` | `7.20` | `27200625` | not sampled |
| `session_pgcr_ingest_legacy` | `7.16` | `31280737` | `2026-06-22 19:06:53 UTC` |
| `session_instance_lookup_hot_legacy` | `7.03` | `14252335` | not sampled |
| `play_sessions` | `3.80` | `4727949` | `2026-06-24 14:58:26 UTC` |
| `session_summary_facts_legacy` | `3.07` | `2129424` | not sampled |
| `session_activity_events` | `1.62` | `3252210` | `2026-06-24 14:58:13 UTC` |
| `session_legacy_migration_work` | `1.50` | `6838938` | not sampled |
| `session_activity_manifest_legacy` | `1.30` | `1911138` | not sampled |
| `session_storage_migration_rows_legacy` | `0.74` | `4315163` | not sampled |
| `session_pgcr_ingest` | `0.41` | `1839522` | `2026-06-24 14:58:11 UTC` |
| `session_achievement_state` | `0.31` | `848815` | not sampled |

The major retained `_legacy` tables alone are roughly `196 GiB`:

- `session_activity_events_legacy`: `102.61 GiB`
- `play_sessions_legacy`: `74.29 GiB`
- `session_pgcr_ingest_legacy`: `7.16 GiB`
- `session_instance_lookup_hot_legacy`: `7.03 GiB`
- `session_summary_facts_legacy`: `3.07 GiB`
- `session_activity_manifest_legacy`: `1.30 GiB`
- `session_storage_migration_rows_legacy`: `0.74 GiB`
- `session_achievement_awards_legacy`: `0.16 GiB`

Do not drop these just because they are large. They need a reviewed cutover-acceptance/replay/rollback decision first.

## Short live write-rate sample

Sample window: 2026-06-24 14:56:33 UTC to 14:58:34 UTC, about 121 seconds.

| Table/key | t0 | t1 | Delta | Approx rate |
|---|---:|---:|---:|---:|
| `session_activity_events.id` | `1000005536196` | `1000005541492` | `5296` | `43.8/s` |
| `session_activity_events_legacy.id` | `51542690` | `51542690` | `0` | `0/s` |
| `play_sessions.id` | `1000000785915` | `1000000786329` | `414` | `3.4/s` |
| `play_sessions_legacy.id` | `6576403` | `6576403` | `0` | `0/s` |
| `session_pgcr_ingest.instanceId` | `16966000371` | `16966013175` | `12804` | `105.8/s` |
| `session_pgcr_ingest_legacy.instanceId` | `16956567135` | `16956567135` | `0` | `0/s` |

Interpretation: current active writes are going to the compact replacement tables, not to the giant `_legacy` tables. The giant `_legacy` tables are durable retained storage, not current write-amplification.

## Active transient storage pressure candidate

At 2026-06-24 15:03 UTC, `information_schema.processlist` showed a long-running query:

```sql
SELECT COUNT(*)
FROM (
  SELECT a.instanceId, COUNT(DISTINCT a.ratingPool) pools
  FROM nexus.skill_rating_match_audits a
  WHERE a.runId IN (16,6,7,8,13)
  GROUP BY a.instanceId
  HAVING pools > 1
) duplicates
```

It had been running for about `578s`. The owning OS process was:

```text
/home/charlemagne/bin/warmind-skillratings -cmd=repair-season -season=30 -pool=crucible_core,srl,gambit,trials -repair-srl -cutoff-live-ingest -execute -workers-paused -allow-exhausted -resume-repair -plan-out=/tmp/wlr-s30-repair-execute-resume.json
```

Observed process start: 2026-06-24 07:20:15 PDT.

This query scans/groups `nexus.skill_rating_match_audits`, which is `81.99 GiB`. It is a plausible source of transient MySQL temp-table disk pressure. MySQL status/variables at the time:

| Setting/status | Value |
|---|---:|
| `Created_tmp_disk_tables` | `3170899` |
| `Created_tmp_tables` | `66814703` |
| `Created_tmp_files` | `598419` |
| `Threads_running` | `12` |
| `Threads_connected` | `156` |
| `tmpdir` | `/tmp` |
| `tmp_table_size` | `16777216` |
| `max_heap_table_size` | `16777216` |
| `temptable_max_ram` | `1073741824` |
| `temptable_max_mmap` | `1073741824` |
| `temptable_use_mmap` | `ON` |

`/tmp` itself was only about `2.1G` when checked, so any earlier large temporary pressure either finished, was elsewhere under MySQL/InnoDB internals, or was deleted-but-still-open. This pass could not prove that without root/PROCESS access.

## Non-MySQL on-disk storage

Accessible filesystem scan:

| Path | Size | Notes |
|---|---:|---|
| `/home` | `62G` | accessible partial scan |
| `/home/charlemagne` | `53G` | not enough to explain the current storage issue |
| `/home/charlemagne/working-production` | `725M` | old manifest ZIP spike has not returned |
| `/home/charlemagne/charlemagne-prod` | `1.8G` | old SQL dump spike has not returned |
| `/home/charlemagne/pkg` | `11G` | Go/module cache, small compared with MySQL |
| `/tmp` | `2.1G` | includes `1.7G /tmp/warmind-cutover-go` and `340M /tmp/warmind-session-tracking-storage-cutover` |
| `/var/tmp` | `0` | no issue |
| `/var/log` | `11G` | logs are not the main issue |
| `/var/cache/yum` | `827M` | small |
| `/var/lib/mysql` | no access | use MySQL metadata or root-side du |

Recent large non-DB files:

- `/var/log/mysql-slow.log`: about `3.0G`
- `/var/log/mongodb/mongod.log`: about `1.84G`
- `/home/charlemagne/logs/nexus-mind.log-2026062407`: about `698M`
- many user journal files at `128M` each
- `/home/charlemagne/dpy_grave.tgz`: about `11.5G`, old from 2022

These are cleanup candidates but not the fast durable bleed.

## Access gaps

This pass could not inspect:

- `/var/lib/mysql` physical files with `du` because the SSH user cannot read it.
- deleted-but-open files with `lsof +L1` because `lsof` was unavailable to this shell.
- `information_schema.innodb_temp_table_info` because the MySQL user lacks `PROCESS`.

Those are the next checks if the free-space line drops sharply again while no table size grows.

## Working conclusion

1. Fastest durable MySQL growth since the last saved baseline is `charlemagne` session/cutover storage. The big durable body is the retained `_legacy` session tables, not current active inserts.
2. Current active session writes are much smaller than the old tables and are going into the compact replacements.
3. The sharp intraday free-space drop/rebound looks more like transient MySQL temp work than a permanent file leak. The active Season 30 repair duplicate-audit query over `skill_rating_match_audits` is the main suspect observed during the pass.
4. Non-MySQL host storage is not the main driver right now. The earlier manifest ZIP and top-level SQL dump problems have not returned.

## Suggested next actions

1. Keep watching `df -h /` and Datadog root free while the `warmind-skillratings repair-season` process is running. If free space falls sharply again, inspect MySQL temp/deleted files with root before killing anything.
2. Review the WLR repair duplicate-audit query shape. Avoid broad `GROUP BY instanceId` over `skill_rating_match_audits` during production repair if a smaller indexed check can prove the same gate.
3. Decide, in a separate reviewed maintenance step, whether the `_legacy` session tables can be archived/dropped after cutover acceptance, replay proof, and rollback criteria are satisfied. Potential table-space target is about `196 GiB`, but logical drops/rebuilds and physical EBS reclaim are separate decisions.
4. Keep non-DB cleanup as background work only: logs, Go cache, `/tmp/warmind-cutover-go`, and the old `dpy_grave.tgz` are helpful but not the core bleed.

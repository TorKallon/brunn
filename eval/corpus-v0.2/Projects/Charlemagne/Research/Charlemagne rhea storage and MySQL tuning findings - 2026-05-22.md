# Charlemagne rhea storage and MySQL tuning findings - 2026-05-22

Updated: 2026-05-22 15:45 PDT

## Related
- [[Projects/Charlemagne/Charlemagne|Charlemagne]]
- [[Projects/Charlemagne/Research/Charlemagne infrastructure research|Charlemagne infrastructure research]]
- [[Projects/Charlemagne/Research/Charlemagne MySQL storage reduction guide - 2026-04|Charlemagne MySQL storage reduction guide]]
- [[Projects/Warmind/Warmind|Warmind]]
- [[Projects/Warmind/Warmind production containerization discovery bundle - 2026-05-17|Warmind production containerization discovery bundle - 2026-05-17]]

## Scope
Live production work on `rhea` covering:

- read-only disk-space attribution across the host and MySQL
- approximate per-table sizing from `information_schema`
- narrowly authorized cleanup of the October 2024 manifest ZIP spike
- narrowly authorized removal of old top-level SQL dumps in `/home/charlemagne/charlemagne-prod`
- code review for whether `nexus.raw_raids` can move to summaries without losing functionality
- read-only snapshot of MySQL file-descriptor settings, active config, and relevant OS tuning

No service restarts, config edits, schema changes, or broad cleanup were performed in this pass. Host inspection stayed read-only until the specific manifest and dump deletions were explicitly authorized.

## Executive summary
- Before cleanup, `/` was about `3.2T` total, `2.8T` used, `377G` free, `89%` used.
- After the scoped manifest and dump cleanup, `/` was about `494G` free and `85%` used.
- The main storage problem is still MySQL, not host logs or package caches.
- The two biggest easy non-DB wins were real and already landed:
  - October 2024 manifest ZIP spike: `3571` files, `81.83 GiB`
  - top-level old SQL dumps in `/home/charlemagne/charlemagne-prod`: about `41 GiB`
- The biggest MySQL footprint is concentrated in `nexus`, especially large summary tables plus `raw_raids` and `raw_dungeons`.
- `nexus.raw_raids` should not be replaced by one summary table. The safe path is summaries plus narrow fact tables, with dual-write and shadow-read validation.
- MySQL is capped at `10000` open files and is running with `Open_tables=2944` against `table_open_cache=2947`, so file and table-cache headroom is tight enough to be worth a planned tuning pass.

## Root disk and non-DB findings

### Main host offenders before cleanup
| Area | Approx size | Notes |
|---|---:|---|
| `/home/charlemagne/working-production` | `97G` | Mostly `D2Manifest_v*.zip` files; the October 2024 spike alone was `81.83 GiB` across `3571` files |
| `/home/charlemagne/charlemagne-prod` | `43G` | Mostly old top-level `.sql` dumps |
| `/var/log` | `9.1G` | Included `journal` about `3.2G`, MySQL slow log about `1.8G`, Mongo log about `1.7G` |
| `/var/cache/yum` | `6.2G` | Standard package cache, not touched in this pass |
| `/home/charlemagne/pkg` | `9.9G` | Go module/build cache, not touched in this pass |

### Other datastore footprints
| Store | Approx size | Notes |
|---|---:|---|
| MongoDB | `21.3G` | Mostly `local.oplog.rs` at about `19.8G`; actual `warmind` DB storage was under `1G` |
| Elasticsearch `players1` | `23G` | About `72.2M` docs and `11.3M` deleted docs |
| Redis memory | `2.33G`, `1.01G`, `25.17G`, `35M` | Observed on ports `6380`, `6381`, `6382`, `6383` respectively |

## Scoped cleanup performed

### 1) October 2024 manifest ZIP spike
- Deleted only `/home/charlemagne/working-production/D2Manifest_v2024-10-*.zip`
- Before: `3571` files, `81.83 GiB`
- After: October 2024 files reduced to `0`
- Remaining manifest ZIPs after cleanup: `1131` files, `14.11G`
- `/home/charlemagne/working-production` dropped from about `97G` to about `19G`
- Root free space improved from about `377G` to about `459G` after this cleanup

### 2) Old top-level SQL dumps
- Deleted only top-level `*.sql` files in `/home/charlemagne/charlemagne-prod`
- Scope was intentionally narrow: files only, no subdirectories, no repo contents
- Deleted files:
  - `charlemagne_db_7_25_20.sql`
  - `seals_gilded_s13.sql`
  - `profiles20210822.sql`
  - `donators8282021.sql`
  - `donations8282021.sql`
  - `events_10312021.sql`
  - `charlemagne_db_7_22_22.sql`
  - `charlemagne_db_3_31_22.sql`
  - `profiles_db_4_1_22.sql`
- `/home/charlemagne/charlemagne-prod` dropped from about `43G` to about `1.8G`
- Root free space improved from about `459G` to about `494G` after this cleanup

## MySQL size findings

### Schema totals
| Schema | Approx size |
|---|---:|
| `nexus` | `1612.11 GiB` |
| `charlemagne` | `53.99 GiB` |
| `nexustoo` | `8.46 GiB` |
| `discord` | `6.09 GiB` |

### Largest observed MySQL tables
| Table | Approx size | Notes |
|---|---:|---|
| `nexus.summary_atime` | `181.884G` | Largest single table in the first pass |
| `nexus.raw_raids` | `178.711G` | About `706M` estimated rows; key structural target, but not a simple cleanup |
| `nexus.raw_dungeons` | `146.970G` | Another very large raw endgame table |
| `nexus.summary_weapon_meta_daily` | `140.843G` | Reported about `38.937G` `data_free`, so likely worth a rebuild/rewrite investigation |
| `nexus.summary_strikes_detailed` | `93.209G` | Large summary family |
| `nexus.summary_gambit` | `89.956G` | Large summary family |

## `raw_raids` review

### Current shape
- `nexus.raw_raids` is about `178.711G` total with about `84.816G` data and `93.894G` index space
- Estimated rows: about `706,447,662`
- Current primary key: `(membershipId, characterId, instanceId)`
- Secondary indexes exist on `membershipId`, `instanceId`, and `characterId`
- The table is not just cold history. It still feeds live product behavior through:
  - `nexusdb/raids.go` `GetEndgameActivityData`
  - `warmindapi/endpoints.go` `/raids/:membershipId`
  - `discord/dcmds/raid.go`
  - `discord/dcmds/pve.go`
  - `warmind/last.go`
  - `nexusdb/carries.go`
  - `sweeperbot/charlemagne.go`
  - `sweeperbot/nexus.go`

### Verdict
`raw_raids` can probably be reduced, but not by collapsing it into one summary table without losing behavior.

The safe direction is:
- `raid_member_activity_summary`
- `raid_character_activity_summary`
- `raid_character_activity_weekly`
- `raid_success_events` or similar narrow success/carry/low-man fact storage

That design preserves:
- per-member totals
- per-character detail for `DetailsByChar`
- current-week per-character detail
- latest and first-success lookups
- carry/sherpa logic
- low-man and analytics support

### Safe migration outline
1. Add dual-write beside `parser/parsenorm.go` raw inserts.
2. Backfill summaries and narrow facts in chunks.
3. Shadow-compare current `GetEndgameActivityData` results against the summary path.
4. Move `/raids`, `/pve`, refresh jobs, and leaderboards to summary reads.
5. Move carry/sherpa and low-man logic to first-success plus success-event facts.
6. Only then consider archiving or dropping old raw rows.

### Small intermediate check worth doing
Because the primary key already starts with `membershipId`, the standalone `membershipId` secondary index on `nexus.raw_raids` may be redundant. The standalone `characterId` index may also be a candidate, but both should be validated against real query usage before any DDL.

## MySQL file-descriptor and tuning snapshot

### Identity and process
| Layer | Setting | Value | Notes |
|---|---|---:|---|
| Host | OS | Amazon Linux 2 | kernel `4.14.355-280.695.amzn2.x86_64` |
| MySQL | version | `8.0.31` | `MySQL Community Server - GPL` |
| systemd | unit | `mysqld.service` | no drop-ins |
| Process | pid | `8706` | user/group `mysql:mysql` |

### File-descriptor and table-cache settings
| Layer | Setting | Value | Notes |
|---|---|---:|---|
| systemd | `LimitNOFILE` | `10000` | from `mysqld.service` |
| `/proc/<pid>/limits` | max open files | `10000` soft / `10000` hard | process cap |
| MySQL config | `open_files_limit` | `9216` | from `/etc/my.cnf` |
| MySQL active | `open_files_limit` | `10000` | active runtime value |
| MySQL active | `innodb_open_files` | `2947` | matches table-cache family scale |
| MySQL active | `table_open_cache` | `2947` | very close to observed `Open_tables` |
| MySQL active | `table_definition_cache` | `1873` | active runtime value |
| MySQL active | `max_connections` | `4096` | high ceiling relative to observed peak |
| MySQL status | `Open_tables` | `2944` | almost at cache cap |
| MySQL status | `Opened_tables` | `295267` | cumulative since startup |
| MySQL status | `Table_open_cache_overflows` | `287802` | cumulative over about `233` days uptime |
| MySQL status | `Max_used_connections` | `751` | much lower than `4096` |

Exact live fd enumeration from `/proc/8706/fd` was not available without root because that directory is only readable by `mysql`.

### Relevant active MySQL settings
| Setting | Value |
|---|---:|
| `innodb_buffer_pool_size` | `103079215104` bytes, about `96 GiB` active |
| `innodb_log_file_size` | `1073741824`, `1 GiB` |
| `innodb_log_files_in_group` | `2` |
| `innodb_flush_log_at_trx_commit` | `2` |
| `innodb_flush_method` | `fsync` |
| `innodb_flush_neighbors` | `0` |
| `innodb_io_capacity` | `3000` |
| `innodb_io_capacity_max` | `6000` |
| `innodb_read_io_threads` | `8` |
| `innodb_write_io_threads` | `8` |
| `thread_cache_size` | `40` |
| `slow_query_log` | `ON` |
| `long_query_time` | `10` |
| `log_bin` | `OFF` |
| `binlog_expire_logs_seconds` | `2592000` |

### Relevant OS tuning
| Setting | Value | Notes |
|---|---:|---|
| `fs.file-max` | `19355915` | system-wide file cap |
| `fs.file-nr` | `14112 0 19355915` | current allocated / unused / max |
| `fs.nr_open` | `1048576` | per-process kernel ceiling |
| `vm.overcommit_memory` | `1` | persisted in `/etc/sysctl.conf` and `/etc/sysctl.d/99-sysctl.conf` |
| `vm.swappiness` | `60` | default-looking value |
| `vm.dirty_ratio` | `20` | active |
| `vm.dirty_background_ratio` | `10` | active |
| `vm.max_map_count` | `262144` | from Elasticsearch sysctl file |
| `net.core.somaxconn` | `65536` | persisted |
| `net.ipv4.tcp_max_syn_backlog` | `3240000` | persisted |
| `net.ipv4.ip_local_port_range` | `1024 65000` | persisted |
| THP | `madvise` | both `enabled` and `defrag` were on `madvise` |
| NVMe scheduler | `[none]` | `read_ahead_kb=128`, `nr_requests=63` |

## Recommended next steps
1. Treat the easy non-DB storage wins as mostly done for now. The next host-side cleanup candidates are much smaller: slow-log rotation, `yum` cache, and any truly dead build caches.
2. Plan a MySQL maintenance/tuning pass for file and table-cache headroom. The current process cap is `10000`, `Open_tables` is almost equal to `table_open_cache`, and cache overflows are non-zero over long uptime.
3. Prioritize the biggest MySQL storage levers in this order:
   - validate whether `summary_weapon_meta_daily` can reclaim a meaningful chunk via rebuild because `data_free` was about `38.9G`
   - decide whether `raw_raids` gets the summary-plus-facts migration project
   - review other top `nexus` summary tables for retention, rebuild, or index strategy
4. Keep `raw_raids` in the "product and schema project" bucket, not the "quick cleanup" bucket.
5. If deeper MySQL internals are needed later, root or more privileged MySQL access would let us inspect exact fd usage, tablespace metadata, and any service-level overrides more deeply.

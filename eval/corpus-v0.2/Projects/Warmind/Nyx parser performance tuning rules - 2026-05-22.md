## Needs From Rourke

| Latest experiment average parse rate | CPU idle | Current PGCR | Current experiment |
|---:|---:|---:|---|
| PVE34 200k 10M-crossing chunk completed; steady avg `403/sec`, late avg `408/sec`, drain avg `372/sec` | Run idle avg `68.7%`, iowait avg `9.5%`; useful band idle stayed about half the machine | `10.019M` | 10M reached and validated. Current bottleneck is still DB/write coordination, not fetch or JSON. Next cleanup candidate: reduce hot-path `latest_parsed` status writes. |

| Need | Why | Status |
|---|---|---|
| Full Warmind dev stack | Frontend validation depends on API, SPA, edge, and supporting containers being back up after parser-only slim-stack experiments. | Restored; use `http://nyx:13091/` |
| Real local D1 S3 env route | If env is missing, use the existing `d1_nexus.s3_pgcr_batches` ledger value as the local source and pass it silently as process env. Never copy bucket values from old session text and never print values. | Resolved for current shell |
| Possible sudo later | Inspecting or raising host/Docker file descriptor, backlog, or kernel socket limits may require admin privileges. | Not needed yet |
| Approval before OS-level changes | Host-wide sysctl, launchctl, or Docker Desktop resource changes can affect other Nyx projects. | Ask first |
| Any remembered production MySQL knobs | Prior D2 production tuning may point directly to the right variables. | Optional, not blocking |

Related: [[Projects/Warmind/Warmind|Warmind]], [[Projects/Warmind/D1 parser milestone 1 task list - 2026-05-21|D1 parser milestone 1 task list]], [[INDEX|Shared knowledge index]]

# Nyx Parser Performance Tuning Rules

Created: 2026-05-22

Use this when tuning Warmind parsers or similar high-throughput workers on Nyx.

## Preferred Experiment Shape

- First measure the current architecture's true ceiling before trying large architecture swaps.
- Let valid experiments reach steady state before judging throughput.
- Do not stop merely because CPU is pegged during ramp. Full CPU saturation is useful evidence when the run is otherwise healthy.
- Stop early only when the run is measuring the wrong thing: repeated transport/config failures, obvious parser correctness failures, DB connection failure storms, data-integrity failures, or protected-state violations.
- For parser work, a hard kill is a valid part of the experiment. Recovery after a messy stop is required behavior, not optional cleanup.
- After any hard stop, prove restart recovery from durable state before continuing speed work.

## Required Visibility

Every serious run should keep a time series, not just one sample:

- parser rate, queue depth, errors, retries, duplicates, terminal markers;
- host CPU user/sys/idle and load averages;
- disk throughput and transfer rate from `iostat`;
- Docker CPU/memory/network/block IO for MySQL, Redis, parser, Cortex, and nearby Warmind services;
- process CPU/RSS for parser, Cortex, MySQL, Redis, and Docker backend;
- MySQL status counters: connection pressure, running threads, commits/rollbacks, fsyncs, row-lock waits, log waits, buffer-pool reads/waits;
- MySQL processlist grouped by command/state.
- Docker/Linux CPU pressure from `/proc/stat`, especially `cpu_iowait_pct`, `procs_running`, and `procs_blocked`, because Docker Desktop hosts MySQL inside the Linux VM and macOS `top`/`iostat` do not expose the same iowait signal.

For D1 scale runs, `dev/scripts/d1-scale-chunk.sh` writes this under each evidence directory in `perf/` through `dev/scripts/d1-host-perf-monitor.sh`.

## Current Parser Bottleneck Read

Added 2026-05-22 after the direct `easyjson` and `github.com/goccy/go-json` 64-worker runs.

- Do not keep `go-json` for speed in the current D1 PGCR path. The clean direct `easyjson` plateau was about `304/sec`; the clean `go-json` plateau was about `249/sec` on the same 64-worker shape.
- With `easyjson`, parser JSON decode is no longer the dominant CPU consumer on Nyx. The parser process stayed under one core while host CPU was full.
- The large current CPU consumers are MySQL plus Docker Desktop overhead. In the clean `easyjson` plateau, MySQL averaged about three container CPU cores, Docker backend services roughly another core, and host system CPU was very high.
- This means the current Nyx ceiling is probably the Docker/MySQL write path for the D2-style per-PGCR design, not Bungie latency and not S3 upload protection.
- S3 protections are not the current 300-to-600/sec lever. They run in Cortex around 1000-PGCR batch rollup; Cortex CPU was tiny compared with MySQL, and the S3 ledger is one row per 1000 PGCRs.
- The validation scaffold table with real current hot-path cost is probably `raw_pgcr_players`, because it adds indexed rows inside the raw PGCR transaction. `raw_pgcr_weapons` and `raw_raids` were zero rows in the latest checkpoint, so they are not current write-volume drivers.
- The next honest A/B test is to restore direct `easyjson`, then run a bounded apples-to-apples live parser experiment with validation-only scaffold writes disabled while keeping real product outputs, raw markers, S3/cache, carry capture, and D2 guards.
- If that A/B test does not move throughput materially, Nyx is likely near its practical clean-parser ceiling around `300/sec` under Docker Desktop, and further large gains probably require native/Linux MySQL, rhea, or a bigger architecture change.

### Easyjson Restored And Scaffold A/B Guard

Added after run `20260522T233650Z`.

- D1 PGCR decode was restored to generated `easyjson`, matching the D2 parser hot path again.
- A guarded switch exists for validation-only scaffold writes, but do not run it against the canonical live D1 DB casually. If `raw_pgcr_players` is skipped for a live range, Milestone 1 summary recompute validation cannot prove that range from scaffold tables without a later rebuild/reparse plan.
- Use the scaffold-off switch only for a bounded A/B with an accepted validation/rebuild plan or an isolated database copy.
- The clean scaffold-on easyjson run under the restored frontend stack reached a steady `253-257/sec` band and completed at `3243592` PGCRs with S3, coverage, summary, membership, D2 guard, and Datadog evidence passing.
- That run still showed the same cost shape: host CPU full, MySQL the dominant container load, parser process under one core, and Docker Desktop overhead visible. It is a valid coexistence baseline, not proof of the absolute all-of-Nyx ceiling.

### JSON Decoder Experiment Trap

Added after Erebus benchmark discussion on 2026-05-22.

- Pure JSON decode benchmarks on Erebus showed `github.com/goccy/go-json` can be much faster than `easyjson` when it is actually using its own struct decoder.
- Be careful: Warmind's existing D1/D2 PGCR types have generated `UnmarshalJSON` methods from `easyjson`. `go-json` honors those custom methods just like the standard library does.
- A naive parser test like `gojson.Unmarshal(pgcrBytes, &ExistingWarmindPGCRType)` may therefore measure `go-json` dispatching into `easyjson` plus extra overhead, not `go-json` replacing `easyjson`.
- The honest JSON experiment is: create a PGCR type copy without generated `UnmarshalJSON`, run `go-json` against that clean type, and compare it to `easyjson` plus naive `go-json` on the existing generated type.
- 2026-05-22 Nyx result before the next 4M scale leg: clean `go-json` was wired into the D1 parser surface by decoding into a clean DTO and converting back to the existing parser-facing type. Cached-corpus microbench, including conversion: clean `go-json` `3074 ns/op`, `5416 B/op`, `6 allocs/op`; naive `go-json` on existing generated type `6883 ns/op`, `6396 B/op`, `23 allocs/op`; easyjson on existing type `4638 ns/op`, `1654 B/op`, `20 allocs/op`.
- Plain-English read: the naive `go-json` path really was the delegation trap; the proper clean DTO path wins in isolation. Do not assume that means the live parser wins by the same amount, because previous live runs showed MySQL, Docker Desktop, and write contention dominating once host CPU is saturated.
- 2026-05-22 4M leg live result with clean `go-json`: 64 workers with validation scaffold enabled reached `4,014,874` PGCRs cleanly. It peaked near `251/sec` early/mid-run, then slowed through the back half and finished around `200/sec` final parser average with no parser errors/retries/duplicates. Host CPU idle became highly variable late in the run while MySQL CPU also dropped at times, so this late segment should not be read as Nyx being fully CPU-bound. Treat it as a separate clue for the next experiment: if workers are full and CPU is idle, look at Bungie/API latency, per-source connection behavior, scheduler variance, or other coordination stalls before changing DB settings.
- Owner direction after the 4M checkpoint: stay on the clean `go-json` implementation for now because the lower decode cost should matter when the parser is not DB-bound. Do not switch back to `easyjson` before the next owner-provided experiment unless the experiment explicitly asks for that A/B.
- 2026-05-22 4M validation correction: D1 launch-corpus PGCR entries omit D2's `timePlayedSeconds` key and provide `activityDurationSeconds` instead. D1 parser time should therefore use `timePlayedSeconds` when present, otherwise fall back to `activityDurationSeconds`. Summary validation must not be trusted if it only proves zero raw player time equals zero summary time.

## Current MySQL EOF Research

- Do not treat `unexpected EOF` or `driver: bad connection` as proof of a parser worker ceiling. Treat it as a DB/client/OS tuning failure until evidence says otherwise.
- Keep separate questions separate: first make the high-worker shape stable, then decide whether it is actually faster than the lower-worker shape.
- Local D1 evidence after the 80-worker failures does not currently point at a simple open-file/table-cache ceiling: the MySQL container has `nofile=1048576`, MySQL reports `open_files_limit=1048576`, `Table_open_cache_overflows=0`, and no max-connection errors.
- Local D1 evidence does point at connection churn and aborted clients: high-worker runs produced rising `Aborted_clients`, MySQL `Connections` climbed quickly, and the Go DB pool still keeps only `6` idle connections while allowing much larger open-connection pools.
- D1 perf monitor now captures `Aborted_clients`, all `Connection_errors_*`, `Open_files`, `Opened_files`, `Open_tables`, `Opened_tables`, `Table_open_cache_*`, `Threads_created`, and `Threads_cached` in every run, not just after the fact.
- D1 DB max-idle connections are now configurable for performance experiments. Keep the D2-style default of `6` unless a run explicitly opts into a larger idle pool.
- OS-level backlog or file-limit changes may still matter, but current evidence does not justify sudo changes yet. Ask before making host-wide `sysctl`, `launchctl`, or Docker Desktop limit changes.

## Rhea Production Context

Added from Rourke on 2026-05-22 for future comparison, not as a change for the active run.

- Rhea/prod D2 used much larger MySQL connection headroom than Nyx: `max_connections=4096`.
- Rhea/prod MySQL file headroom was still finite: systemd `LimitNOFILE=10000`, MySQL active `open_files_limit=10000`, configured `open_files_limit=9216`.
- Rhea/prod table cache was effectively full after long uptime: `Open_tables=2944` against `table_open_cache=2947`, with nonzero `Table_open_cache_overflows` and `Table_open_cache_misses`.
- Rhea/prod InnoDB settings included `innodb_buffer_pool_size=98000M`, `innodb_buffer_pool_instances=8`, `innodb_log_file_size=1024M`, `innodb_log_buffer_size=248M`, `innodb_read_io_threads=8`, `innodb_write_io_threads=8`, `innodb_io_capacity=3000`, `innodb_change_buffering=all`, `innodb_autoinc_lock_mode=2`, and `transaction-isolation=READ-COMMITTED`.
- Rhea/prod durability was looser than Nyx's current conservative dev profile: `innodb_flush_log_at_trx_commit=2`; Nyx has intentionally kept flush settings conservative so far.
- Rhea/prod OS/network settings had high backlog/headroom values including `fs.nr_open=1048576`, `net.core.somaxconn=65536`, `net.ipv4.tcp_max_syn_backlog=3240000`, local port range `1024 65000`, and `tcp_fin_timeout=15`.
- Future Nyx comparison questions: whether `READ-COMMITTED`, `innodb_autoinc_lock_mode=2`, or looser flush behavior materially improves parser write contention; whether Nyx table/open-file counters ever resemble Rhea's full table-cache state; and whether host/Docker backlog or fd settings need planned sudo changes.

## Bottleneck Reading

- If CPU idle stays high, do not call the machine maxed out. Look for Bungie latency, DB write cadence, Redis/cursor overhead, S3/object work, validation, or coordination overhead.
- If CPU idle is near zero and parser errors stay clean, let the run settle and then inspect MySQL, IO, Redis, and process CPU before changing architecture.
- If MySQL CPU dominates, check row-lock waits, running threads, fsync/write rates, max connections, buffer-pool pressure, and whether parser workers are oversubscribing the DB.
- If high-worker parser runs produce `unexpected EOF` or `driver: bad connection` from MySQL, check DB capacity before judging parser throughput. In the D1 tuning pass, the invalid 96-worker run hit MySQL with `Max_used_connections` close to the old `max_connections`, a tiny thread cache, buffer-pool wait pressure, and historical redo-log starvation warnings. The first fix was to raise connection/thread/backlog headroom plus InnoDB buffer pool, redo capacity, and IO capacity while keeping durability settings unchanged.
- Treat MySQL container OOM as a separate invalid measurement. A 4 GiB buffer pool plus 4 GiB redo capacity with high connection headroom was too large for Nyx's current Docker memory allocation and the MySQL container was killed with exit `137`. Prefer a smaller first stable profile, then increase one resource at a time.
- Distinguish server capacity from client timeout pressure. In the D1 tuning pass, 80 workers still produced MySQL EOF/bad-connection errors even after `max_connections` headroom was ample; the parser host wrapper was still using 10 second DB read/write timeouts while Cortex used 60s/30s. The next tuning step was to make the parser scale path use 60s/30s and increase MySQL buffer-pool headroom without relaxing durability.
- Distinguish MySQL profile memory from total Docker VM memory. A run can OOM even with a reasonable MySQL memory profile if unrelated dev containers consume the remaining Docker Desktop memory. For D1 parser max-rate experiments, prefer a slim Docker stack: MySQL, Redis, Redis cluster, Redis host bridge, and only services required by the experiment. Stop Warmind API/SPA/edge/web, D2 workers/bot/commands, Mongo, and Elasticsearch unless the run explicitly needs them.
- If a higher-worker run still fails after server headroom and DB client timeouts are raised, treat that as evidence of current write-path contention rather than a trustworthy throughput number. Bracket downward between the last clean run and the first failing run.
- If network/source-bind errors appear, that run is invalid for throughput tuning; fix the transport/IP pool before interpreting rates.
- Treat parser worker count and IPv6 source-address pool size as separate knobs. Worker count can exceed the address pool, but the pool size itself must not exceed addresses actually assigned and bindable on Nyx. The host parser wrapper should fail fast by probing both the first and last requested IPv6 source address before starting the parser.

## Split Fetch/DB Pipeline Notes

- 2026-05-22 result: separating fetch concurrency from DB/process concurrency is promising. With validation scaffold still enabled, the same 64-fetch front end reached much higher rates when the DB side was tuned separately.
- Initial ladder: `12` DB workers was too narrow (`~207/sec` late, DB queue full), `24` DB workers was better (`~260-264/sec`), `32` DB workers held a clean `~305-310/sec`, `40` DB workers was too many and sagged to `~260/sec` after an early peak, and `36` DB workers reached late samples around `386-399/sec` on a 50k probe.
- Plain-English read: the front end can keep Bungie/S3 payloads ready while the DB side stays narrow enough to avoid the worst contention. Too narrow starves throughput; too wide makes MySQL trip over its own commits.
- Longer 250k proof: `64 fetch / 36 DB` held a stable `386.00/sec` average over the elapsed `94s-640s` window, with a `381.43-389.52/sec` range and no parser errors/retries/duplicates. Validation passed: 262 S3/coverage windows, summary recompute on retry, membership recompute, D2 Redis guard, and Datadog.
- Current read: `64 fetch / 36 DB` is the best candidate for the 10M push. Fetch is not currently starved because the DB queue stays full; the next tuning question is whether DB-side specialization or summary/scaffold changes can reduce DB contention, not whether more fetch workers are needed.
- If S3 replay leaves host idle high, the limiter is probably not Nyx raw CPU.
- Do not run full summary or membership validation concurrently with parser throughput experiments. On 2026-05-22, a `64 fetch / 36 DB` follow-up appeared to degrade badly, but MySQL processlist showed multiple full-table summary validation SELECTs running for minutes against `raw_pgcr_players` at the same time. That made the run invalid as a parser measurement. Keep validation scaffold writes enabled, but run the heavy validators after the hot parser window unless the experiment is specifically measuring validation load.
- Clean retest after disabling heavy concurrent validation: `split-64f-36db-clean-rollup-250k-20260523T031601Z` averaged about `409/sec` and inserted `262284` raw rows, ending at `4930530`. Late interval rates were mostly `360-450/sec`, with no parser errors, retries, duplicates, or terminal markers. Post-run proof passed S3, coverage, summary, membership, D2 Redis guards, and exact Datadog user-space log verification. Host idle during the parser phase averaged about `40.0%`; Linux/Docker idle averaged about `49.6%` with `9.1%` iowait. Plain-English read: the split pipeline is a real like-for-like improvement, and Nyx still has CPU headroom while the DB queue remains full.
- Invalid follow-up: `split-64f-44db-clean-rollup-150k-20260523T033933Z` is not a valid tuning result. It initially climbed to only about `310/sec`, already below the `36 DB` clean run, then cache high-water paused new work because the concurrent Cortex rollup had a wrong S3 bucket value and could not upload batches. Do not use this run to judge `44 DB`; first restore the real D1 S3 env, drain cache, then retest only if still useful.
- S3 route recovery: if the shell lacks the D1 S3 bucket env, recover it from the owned D1 S3 ledger in `d1_nexus.s3_pgcr_batches` and pass it silently to the parser/Cortex process. Do not select candidates from old session text, do not print bucket values, and do not run scale work while S3 rollup is pointed at the wrong target.
- Full-tooling `38 DB` proof: `split-64f-38db-fulltooling-10k-20260523T040109Z` drained S3/cache first, then ran `64 fetch / 38 DB` with DB max/idle `38/38`, concurrent S3 rollup, and validation scaffold writes enabled. It inserted `17424` rows and validated cleanly, but it was too short for a steady-state throughput conclusion: samples climbed from `143.5/sec` to `268.8/sec` while stop/drain was already happening. Treat this as a correctness proof for the full-tooling shape, not as evidence that `38 DB` is slower or faster than `36 DB`.
- Full-tooling `38 DB` 250k run: `split-64f-38db-fulltooling-250k-20260523T041139Z` inserted `262336` raw rows and ended at `5,297,108` PGCRs. The useful steady band was about `376-386/sec`, with a final stopped/drain cumulative near `348/sec`. Parser errors, retries, duplicates, and terminal markers stayed at `0`. CPU still had headroom: Linux/Docker idle was roughly `45-51%` with `7-10%` iowait, while MySQL was the dominant container load at roughly `240-286%` CPU. Plain-English read: `38 DB` works, but it did not beat the cleaner `36 DB` result; the queue stayed full while CPU remained idle enough that the next question is DB write cadence and row contention, not fetch starvation.
- The parser does not yet have separate PVE and PVP DB queues. The current experiment has a high-concurrency fetch side and one normalized DB/process queue. PVE/PVP lanes are a reasonable next architecture experiment once mode classification and instrumentation are solid, but the current early D1 corpus is overwhelmingly PVE, so separating only PVP from PVE may not help much until there is enough PVP volume. Finer partitioning by activity/reference ID or summary family may matter more for the current launch-corpus shape.
- 2026-05-22 22:10 spot check: the mode classification repair is producing Crucible summaries now (`summary_crucible=261950`, `summary_crucible_overall=34`), and player-seconds totals are nonzero in all current global/activity summary rows. Separate PVE/PVP DB queues still do not exist; use the instrumentation run first, then decide whether to split DB workers by PVE/PVP, finer PVE mode/reference buckets, or summary family.
- 2026-05-22 22:33 instrumented run: `64 fetch / 36 DB` with normal S3 rollup reached a clean hot plateau around `398-405/sec`, then sagged into the `340s/sec` cumulative late in the run while queues stayed full, parser errors remained zero, CPU idle stayed high, and iowait/row-lock waits rose. Next experiment should implement separate PVE/PVP DB lanes, tune those worker counts independently, and then decide whether finer PVE partitioning is needed. For the 10M push, PVE validation should be the summary gate until Crucible summary correctness is separately reviewed.

## D1 DB Contention Notes

Added after the `5.297M` checkpoint.

- The current launch-corpus shape is heavily PVE: Story dominates, followed by Patrol and Strikes. PVP exists but is much smaller in the current window.
- The hottest write pressure is likely not raw PGCR markers themselves. It is the repeated summary upserts into a small number of rows, especially `summary_activity` and `summary_global_activity` for Story/Patrol on the same early launch days.
- `summary_atime` and `summary_time` are more spread out by membership and day, but they still update for every PGCR and can add lock/commit pressure.
- `raw_pgcr_players` adds large indexed insert volume and remains the main validation scaffold write cost. It is useful for Milestone 1 proof, but it is not free.
- `raw_pgcr_weapons` and raid/carry tables currently add no live write volume because sampled D1 PGCR payloads have no weapon rows and the current parsed range has no raid rows.
- Separate PVE/PVP DB queues could reduce cross-family contention later, especially for weapon/meta writes once those are producing rows. For the current early PVE-heavy corpus, the first likely gains are from better measurement, then possibly PVE partitioning or summary-family batching.

## Next Instrumentation Plan

Add timing, counters, and queue-depth visibility before the next scale experiment:

- Fetch stage: source used, fetch latency by cache/S3/Bungie, retry lane counts, S3 batch expansion time, and DB queue blocking time.
- Decode/process stage: JSON decode time, DTO conversion time, row extraction time, rough mode family, and payload size.
- DB stage: queue wait age, total transaction time, rows written, retry/deadlock counts, and per-family timing for raw marker, raw players, raw weapons, raids/carries, base summaries, atime, crucible, strikes/nightfalls, weapon summaries, Tor_Kallon sightings, and failed cleanup.
- Redis stage: call counts and latency for parser status, stats, queue/backpressure, and rollup pointers.
- Locking: summary lock wait time and hold time by lock family.
- System view: continue recording CPU idle, iowait, load, process CPU/RSS, MySQL processlist/state, row-lock waits, commits, fsync/log waits, and queue depths in the evidence directory.
- Report the same parser-rate samples beside stage timings so worker-count changes can be explained by what got faster or slower, not just by the final PGCR/sec number.

2026-05-22 implementation note:

- Parser metrics now time fetch-source lookup by cache/S3/Bungie, fetched payload bytes, S3 batch read/decode/expansion, DB handoff blocking, DB queue wait, cache save, JSON decode, row extraction, mode check, profile/character upserts, every major transaction stage, terminal/failure marker stages, DB parser-state writes, Redis status/stat calls, and total DB/process work.
- Summary metrics now time and count finite summary families: base, atime, crucible, strikes, nightfalls, player weapons, weapon meta, Trials weapon meta, and raid analytics.
- Summary lock metrics now record wait and hold time by finite lock family, including crucible/strikes/nightfalls/raids/player-weapons/weapon-meta/Trials-weapon-meta.
- Queue depth/capacity gauges already existed for fetch, retry-fetch, and DB queues; the new handoff-block timing explains when fetch workers are stuck behind a full DB queue.
- The next scale run should keep the same parser-rate and host/MySQL monitor outputs, then compare those rates against `parser.d1_tx_stage`, `parser.d1_summary_part`, `parser.d1_db_queue_send_block`, `parser.d1_db_queue_wait`, and `parser.d1_summary_lock_*` to decide whether the next useful split is PVE/PVP lanes, finer PVE partitioning, or summary-family batching.

## D1 Weapon Source Notes

Added after the `5.297M` checkpoint.

- Current D1 weapon/meta tables are still empty because no parsed payload has produced weapon rows.
- The parser is wired for `entry.extended.weapons` when that section exists, and unit fixtures prove that extraction path works.
- Local cache evidence: all `108` current cached D1 PGCR JSON files lack both `extended` and `weapons`.
- S3 batch sampling was attempted without printing the bucket, but the shell's AWS CLI currently has no credentials, so owned-S3 payload sampling still needs to be retried through the normal configured app path or a shell with AWS credentials.
- Live API spot probe through the repo's D1 client found no weapon detail for the sampled player's D1 ActivityHistory response, and sampled HistoricalStats group probes did not expose weapon keys.
- Plain-English read: this now looks more like a D1 data-source/API limitation or a different endpoint requirement than a simple parser typo. Do not assume `summary_weapon_meta` should be nonzero until we identify a D1 source that actually exposes per-activity weapon kills.

## Recovery Proof

After a hard stop or invalid run:

- verify no active parser/Cortex process remains;
- verify sentinels, locks, and run-control files are safe;
- treat raw PGCR gaps immediately after a hard stop as normal transient state, not a failure by itself;
- historical D2 production behavior at high concurrency was exactly this: violent parser stops routinely left gaps, and normal startup recovery was the correctness mechanism;
- restart the parser normally and let startup gap scanning queue missing PGCRs while normal parsing continues;
- use gap-repair-only only as a narrow diagnostic or surgical tool, not as the default recovery proof;
- only treat gaps as a blocker if they persist after a normal restart has had a fair chance to scan and work them, or if the parser advances the durable contiguous cursor past unrepaired gaps;
- if concurrent validation records a coverage failure while normal startup recovery is still repairing transient gaps, do not treat that stale row as a current failure once a later same-window coverage pass and direct DB proof supersede it;
- verify `raw_failed_pgcrs` is zero or fully explained;
- verify cache tail and S3 complete windows;
- verify Redis cursors recover from DB if needed;
- verify protected D2 Redis state is unchanged;
- record Datadog visibility and tracker evidence.

The desired contract is D2-style: a parser can die in a messy state, transient gaps can exist, and the next normal start recovers from durable truth without special hand cleanup.

## Parser Cursors

- D2 parser correctness does not depend on a maintained contiguous progress cursor. It uses durable DB truth: start from `MAX(raw_pgcrs.instanceId)`, scan the raw table for gaps, queue those gaps, and continue.
- D1 keeps `next_pgcr_cache_rollup` as the true Cortex operational pointer for 1000-PGCR cache/S3 rollup work. `latest_parsed` remains status. `highest_parsed` has been removed from the D1 parser/Cortex hot path because it was an unnecessary out-of-order failure mechanism.
- If a contiguous high-water value is needed for partition or status work, compute it from `raw_pgcrs` at that boundary instead of writing it per PGCR.

## DB Lane Experiment

Added 2026-05-22 before the next D1 scale run.

- The D1 split parser now has optional DB/process lanes for `pve`, `pvp`, and `other`, with independent worker counts and per-lane/per-shard queue metrics.
- Optional membership sharding can further split each lane. When sharding is enabled, the parser chooses a stable membership key from the PGCR and routes that whole PGCR to one shard, with one DB worker per shard. This should reduce same-player summary write overlap, but it is not a perfect player lock because one PGCR can contain multiple players.
- First test ladder: smoke lane routing without membership sharding, then compare lane-only against membership-sharded PVE. Use the previous instrumented `64 fetch / 36 DB` run as the baseline: hot plateau roughly `398-405/sec`, late sag into the `340s/sec` cumulative, high CPU idle, rising iowait, and visible row-lock waits.
- Judge this experiment by parser rate after steady state, DB queue depth, queue wait, DB transaction stage timing, summary lock wait/hold, row-lock waits, iowait, MySQL CPU, parser errors/retries/duplicates/terminal markers, cache/S3 rollup health, D2 Redis guards, and Datadog visibility.
- Lane-only smoke result: `64 fetch / PVE 34 / PVP 2 / other 1`, no membership sharding, inserted `29632` rows and ramped to `301/sec` while draining with no parser errors. This proves the lane routing path works, but it is too short to judge throughput. Next run should be long enough to stabilize before comparing against the `398-405/sec` baseline.
- Lane-only 250k result: same worker shape inserted `259648` rows, finished at `5.849M`, and validated cleanly. Late useful band was roughly `377-392/sec`, with zero parser errors and Linux/Docker CPU still mostly idle. This is close to the old shared-queue result but not better enough to call a win.
- Membership-sharded 250k result: `34` membership shards inserted `259136` rows and finished at `6.108M`, validating cleanly. It peaked around `434/sec`, but over the full steady run it sagged to about `325/sec`, with parser-window idle `59.8%`, iowait `14.9%`, and repeated MySQL `waiting for handler commit` states. Plain-English read: membership sharding is implemented and safe, but this `34`-shard profile made the current mostly-PVE run slower after the early burst. Keep the knob for later mixed-data tests, but disable it for the next long parse unless new evidence changes the read.
- PVE36 lane result: with membership sharding off and PVE/PVP/other workers `36/2/1`, the parser inserted `259648` rows, finished at `6.368M`, and validated cleanly. The run peaked around `447/sec`, dipped into the high `380s`, then recovered to a final cumulative sample near `397/sec`. Linux/Docker idle still averaged about `60%`, and iowait averaged about `12%`. Plain-English read: matching PVE workers to the old shared-queue DB count removes the lane-only regression and keeps the new PVE/PVP separation without obvious throughput cost.
- PVE38 lane result: with membership sharding off and PVE/PVP/other workers `38/2/1`, the parser inserted `259648` rows, finished at `6.627M`, and validated cleanly. It peaked around `456/sec`, then steadily sagged to about `368-373/sec` while iowait climbed; parser-window Linux/Docker idle averaged `55.1%` and iowait `13.3%`, and late-window iowait averaged `18.0%`. Plain-English read: the extra PVE DB workers create more waiting than useful work on this slice, so PVE38 is not the keeper profile. Use PVE36-style lanes with membership sharding off for the next long push unless the data mix changes enough to justify another comparison.
- PVE36 longer-run result: `split-lanes-64f-pve36-pvp2-other1-500k-to10m-20260523T071658Z` was intentionally stopped with SIGTERM after the rate had clearly decayed. The parser inserted `258496` rows, finished at `6.886M`, and then passed post-parse coverage, summary recompute, membership recompute, D2 Redis guard, and Datadog proof. It peaked around `473/sec`, held about `471/sec` for a short early band, then steadily dropped; late average after elapsed `560s` was about `352/sec`, final drained sample was about `333/sec`. Parser-window Linux/Docker idle averaged `55.8%` and iowait `15.3%`; late-window idle averaged `60.2%` and iowait `20.5%`; row-lock current waits were around `700+`. Plain-English read: PVE36 is correct and safe, but it is too wide for this long mostly-PVE launch slice. The next tuning probe should reduce PVE DB workers rather than increase them.
- PVE30 result: `split-lanes-64f-pve30-pvp2-other1-250k-to10m-20260523T073811Z` inserted `259648` rows and finished at `7.145M`, validating cleanly. It peaked around `454/sec`, but late average fell to about `339/sec` and final drain was about `319/sec`. Plain-English read: fewer PVE DB workers delayed pressure but did not improve the useful sustained rate.
- MembershipId-sharded result: `split-lanes-membership30-64f-pve30-pvp2-other1-250k-to10m-20260523T075954Z` inserted `259264` rows and finished at `7.405M`, validating cleanly. The parser peaked around `340/sec`, mid steady averaged about `325/sec`, and late averaged about `302/sec`. Plain-English read: membership sharding is implemented and safe, but it is slower on the current mostly-PVE launch slice and leaves more machine idle. Keep it as a knob for later mixed-data tests; leave it off for the 10M push.
- PVE36 current best confirmation: `split-lanes-64f-pve36-pvp2-other1-250k-to10m-20260523T082324Z` inserted `259904` rows and finished at `7.664M`, validating cleanly. The steady plateau averaged about `464/sec`, late average was about `462/sec`, and final drain was about `452/sec`. Plain-English read: this is the best measured profile so far.
- PVE36 500k slice result: `split-lanes-64f-pve36-pvp2-other1-500k-to10m-20260523T084117Z` was stopped after DB waits built up. It inserted `111168` rows and finished at `7.776M`, then passed manual recovery/validation. Plain-English read: longer slices let row-lock/handler-commit waits pile up, so 500k is not the right chunk size for this data band.
- PVE36 250k continued result: runs `20260523T085634Z` and `20260523T091641Z` finished at `8.036M` and `8.296M`, validating cleanly, with useful plateau around `445-461/sec`.
- PVE36 250k late-cliff result: `split-lanes-64f-pve36-pvp2-other1-250k-to10m-20260523T093752Z` inserted `260480` rows and finished at `8.557M`, validating cleanly, but the rate decayed from an early `441/sec` average to a late `330/sec` average and a final pre-drain sample near `276/sec`. Linux iowait climbed to about `23%` late and MySQL showed many `waiting for handler commit` states. Plain-English read: PVE36 is still the best worker shape, but chunk length matters; a shorter run window may avoid the late DB-wait cliff while still preserving the same steady-state rate.
- PVE36 200k chunk-length probe: `split-lanes-64f-pve36-pvp2-other1-200k-to10m-20260523T100517Z` inserted `210432` rows and finished at `8.767M`, validating cleanly. It peaked around `450/sec`, but after the peak band the cumulative rate fell steadily; the late parser window averaged about `366/sec`, and the drain average was about `297/sec`. Parser-window idle averaged `54.0%` with `15.7%` iowait; late idle averaged `59.5%` with `20.7%` iowait. Plain-English read: shorter chunks do not remove the hot-band DB wait. Keep the split fetch/DB architecture and lane knobs; compare PVE34 in the same range before declaring PVE36 final for the current data shape.
- PVE34 same-band comparison: `split-lanes-64f-pve34-pvp2-other1-200k-to10m-20260523T102811Z` inserted `210432` rows and finished at `8.978M`, validating cleanly. Steady average from elapsed `250-461s` was about `425/sec`; late average from `300-461s` was about `428/sec`; drain average was about `383/sec`. Parser-window idle averaged `48.8%` and iowait `10.2%`; late idle averaged `48.9%` and iowait `10.4%`; MySQL averaged `256.7%` CPU. Plain-English read: PVE34 is currently better than PVE36 in this hot band because it gives up a little peak speed but avoids the handler-commit/iowait cliff. Try PVE34 at `250000` next; if it stays smooth, use it for the remaining 10M push.
- PVE34 250k keeper run: `split-lanes-64f-pve34-pvp2-other1-250k-to10m-20260523T104911Z` inserted `260800` rows and finished at `9.239M`, validating cleanly. Steady average from elapsed `220-555s` was about `447/sec`; late average from `400-555s` was about `450/sec`; drain average was about `405/sec`. Parser-window idle averaged `47.5%` and iowait `10.0%`; late idle averaged `48.6%` and iowait `11.0%`; MySQL averaged `266.4%` CPU. Plain-English read: PVE34 held a 250k slice without the DB-wait cliff, and is the current best practical profile on Nyx. Continue toward 10M with PVE34, membership sharding off.
- PVE34 500k probe after the keeper run: `split-lanes-64f-pve34-pvp2-other1-500k-to10m-20260523T111145Z` was hard-killed after the rate had clearly degraded. It started well, with a useful steady band around `431/sec`, but the late average fell to about `376/sec` and the final sample before kill was about `339/sec` while DB queue stayed high and host iowait spiked. Plain-English read: a single 500k slice still lets DB wait accumulate, even with PVE34. This is not a correctness failure; it is a chunk-shape/performance finding.
- PVE34 hard-kill recovery and follow-up chunk: the next normal parser start `split-lanes-64f-pve34-pvp2-other1-250k-to10m-20260523T112945Z` scanned `raw_pgcrs`, found `1,336` gap ranges / `1,376` missing IDs from the kill tail, queued them automatically, and continued normal parsing. It inserted `259744` rows, finished at `9.809M`, and validated cleanly. The useful steady window averaged about `395/sec`, late averaged about `336/sec`, and the final state was count-contiguous through `9,808,601`. Plain-English read: D2-style messy-stop recovery works; no manual gap repair was needed.
- New likely hot-path cleanup: MySQL processlist during stop/drain showed frequent `parser_state.latest_parsed` writes. That value is status-only, not correctness. `highest_parsed` is already removed from the hot path; consider demoting or rate-limiting `latest_parsed` writes too, because a per-PGCR status write can add commit pressure without protecting data.
- PVE34 10M crossing run: `split-lanes-64f-pve34-pvp2-other1-200k-to10m-20260523T115711Z` inserted `210624` rows and finished at `10.019M`, validating cleanly. Steady average from elapsed `220-490s` was about `403/sec`; the late pre-stop band averaged about `408/sec`; drain averaged about `372/sec`. It crossed `10,000,000` at elapsed `442s` with `0` parser errors, retries, duplicates, or terminal markers. Plain-English read: PVE34 remains the best practical Nyx profile in this data band, but there is still a lot of idle headroom because the write path, not fetch, is the limiter.
- 10M validation read: summary recompute passed for `1-10019225`, sampled membership recompute passed, S3 and coverage windows passed for the final chunk, and protected D2 Redis counts stayed unchanged. PVE raw-player totals match `summary_activity` and `summary_time` exactly for activities, successes, time played, kills, deaths, and assists. Nightfall summaries are empty because mode `16` has zero raw rows through this range. Weapon/meta tables remain empty because no D1 weapon rows have been sourced yet.

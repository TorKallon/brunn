Created: 2026-05-25 08:55 PDT
Updated: 2026-05-25 08:55 PDT
Status: durable context summary
Repo: /Users/Shared/projects/warmind-speculative
Branch: codex/d1parser-milestone-1
Related: [[Projects/Warmind/Warmind|Warmind]], [[Projects/Warmind/D1 parser|D1 parser]], [[Projects/Warmind/D1 parser validation and autotuning scenarios - 2026-05-24|D1 parser validation and autotuning scenarios]], [[Projects/Warmind/D1 parser 200M autoscaling tracker - 2026-05-24|D1 parser 200M autoscaling tracker]]

## Purpose

This note captures the lessons, thinking, code-change summary, and evidence from the intensive D1 parser autonomy/autotuning work around 2026-05-24 and 2026-05-25.

The durable theme: make the D1 parser more automatic without making it less safe. The parser should adapt fetch, feeder, batch, DB/write lanes, and rollup/S3 workers based on evidence, while preserving the D2-shaped integrity model: DB rows are truth, raw marker commits travel with summaries, and normal parser restart fills gaps.

## North Star

- Fully autonomous parsing is the goal, not a pile of magic worker-count knobs.
- Data integrity beats throughput.
- Worker counts are evidence, not identity.
- Autoscaler state is optimization state, never correctness state.
- Restart recovery must remain ordinary parser behavior, not a rebuild, replay, or special gap-fill mode.
- Cortex must not enter the parser DB write path. Parser-owned S3 reads belong in the parser fetch path.
- D2 protected Redis namespaces stay read-only.
- Do not write secrets, token values, production bucket names, or AWS key values into notes, logs, commits, or responses.

## Current State Snapshot

| Scope | Best or Latest Evidence | Rate / Outcome | Meaning |
| --- | --- | --- | --- |
| Best current-code alpha rate | 20:00 fixed-ceiling alpha, 500k requested | avg `1083.1 PGCR/sec`; steady `1122.9/sec`; final/max `1201.8/sec` | Best clean current-code speed proof so far |
| Best current-source no-special-fetch alpha | 02:30 alpha, 2M requested | avg `1056.4/sec`; last-half `1092.8/sec`; final `1077.1/sec` | Strong proof that fetch can step below `112` and hold rate |
| Latest corrected no-special-fetch proof | 07:58 alpha, 1M requested | avg `952.0/sec`; after-six `991.5/sec`; last-half `1034.0/sec`; max `1102.4/sec` | Controller reduced to `32`, rolled back, and raised again on real underfeed |
| Best gamma S3-source proof | 21:22 gamma, 1M staged gap | avg `2141.3/sec`; steady `2243.0/sec`; final/max `2469.0/sec` | Parser-owned S3 can feed DB writes very fast; DB lane tuning matters |
| Best beta latency proof | sustained `760ms` artificial latency | steady around `125-131/sec`; fetch scaled to `128` | Correct upstream-limited behavior; do not add DB workers for latency |
| Delta slow-edge proof | forced-lag edge test | fetch/retry reduced to floor `4` and held | Scarcity can be intentional; do not fan out workers at the front edge |
| MySQL capacity proof | controlled post-OOM alpha, 2M requested | avg `1001.1/sec`; MySQL memory stayed under prior OOM edge | 3GiB dev buffer pool stabilized Nyx for 2M scale |

## Major Code Changes

### Autoscaler

- Added the D1 parser autoscaler under `d1parser/autoscaler.go` with focused tests in `d1parser/autoscaler_test.go`.
- Added active and shadow decisions for fetch workers, DB lane rebalancing, batch decisions, cooldowns, rollback, and bad-trial memory.
- Removed source-level `112` semantics completely. There is no backparse seed, no special class, no target, and no floor tied to `112`.
- Added the "smallest useful fetch pool" behavior:
  - scale down under DB/cache/retry/slow-edge/backpressure conditions;
  - scale down for efficiency when sustained rate is healthy and queues are fed;
  - scale back up when current queued fetch work plus low downstream pressure proves real underfeed.
- Added path-independent fetch recovery. A lower accepted fetch count can become too small later; recovery now uses present underfeed and pressure evidence rather than the path that reached the count.
- Added hot-lane-aware fetch-up blocking. If one DB lane is loaded, the controller treats that as DB pressure even if total DB queue percentage looks modest.
- Added broader DB rebalance bad-trial memory. Repeated alpha `35/6 -> 36/5` trials are suppressed after rollback for comparable shapes, while gamma/S3 abundance can still try that target because it is a different scenario.

### Worker Management

- Added dynamic worker-manager plumbing in `d1parser/worker_manager.go`.
- Workers can scale up and down without closing shared queues.
- Scale-down retires a worker after its current PGCR finishes.
- Static DB lanes and dynamic activity lanes use the same worker-manager model.

### Batch Autotuning

- Added batch decisions for warmup, stop approach, underfeed, and DB pressure.
- Settled current policy at batch `128` as the learned keeper-family ceiling.
- Smaller batches may rise toward `128`; exploration above `128` needs an explicit above-keeper trial path.
- Stop-drain and near-stop queue drain are not tuning evidence.
- Hot DB lane pressure blocks "increase batch for underfeed" recommendations.

### Rollup / S3

- Reworked PGCR cache/S3 rollup in `d1cortex/rollups.go` into a feeder plus bounded worker pool.
- Added rollup telemetry for queue depth, worker count, batches/sec, PGCR/sec, latency, failures, retries, and drain behavior.
- Preserved 1000-PGCR S3 batch economics.
- Added idempotent skip/recovery behavior for already-verified S3 batches.
- Removed the rejected Cortex S3 replay concept and kept parser-owned S3 reads inside the parser fetch path.

### Parser-Owned S3 Source

- Added explicit S3 batch-feed mode for gamma tests.
- Startup gaps can be queued as S3-backed range jobs when the mode is enabled.
- S3 range jobs split on 1000-PGCR batch boundaries so fetch workers can fan out while preserving batch economics.
- Stop-drain can discard uncommitted in-memory fetch/retry jobs, then drain DB queues that already own fetched PGCRs.
- The next normal parser run resumes from DB truth for any uncommitted tail work.

### Slow Edge / Delta

- Added forced-lag slow-edge simulation and pause controls.
- Added pause sleep/cadence so the parser does not hot-loop while intentionally paused.
- Normal fetch workers wait through real-time pause instead of recording pause as a retry error.
- Slow-edge policy can reduce fetch/retry workers to the configured floor.

### Integrity / Recovery

- Removed rejected scaffolding and crutch concepts:
  - no `summary_time`;
  - no `raw_pgcrs_players`;
  - no D1 rebuild/recompute path;
  - no parser-state correctness table;
  - no Cortex S3 replay into DB writes.
- Failed rows loaded at startup now retry immediately.
- D1 host/dev retry behavior was made more conservative and D2-shaped.
- Consecutive cache-stat/DB availability failures now trigger a parser stop reason instead of spinning as cache pressure.
- Cache high-water no longer deadlocks the parser away from the one normal raw commit needed to complete the rollup head.
- Datadog user-space submit/verify now happens before final readiness when the Agent file-log config is missing.

### Population Analytics And Calendar

- Added a shared `popanalytics` package.
- Added D1 population recorder and rollup wiring:
  - D1 parser recorder;
  - D1 Cortex daily/weekly rollup;
  - D1 Nexus DB persistence helpers;
  - D1 population status script.
- D1 population keys use `nexus_d1:pop:*` and do not reuse `nexus_d1:latest_parsed`.
- D1 reset day/week helpers use the fixed Destiny 1 reset cycle.
- Parser summaries, lock keys, and population rollups use the D1 calendar rather than D2 reset assumptions.

### Scripts, Config, And Ops

- Expanded `dev/scripts/d1-scale-chunk.sh` with scenario controls, confirmation gates, evidence capture, Datadog ordering, cache high-water, S3 batch-feed, artificial latency, forced lag, pause cadence, autoscaler controls, batch controls, and rollup controls.
- Updated parser/Cortex host-run scripts for the new config surfaces.
- Added population status tooling.
- Added/expanded host perf monitoring for CPU, MySQL, locks, memory, and long-soak pressure.
- Lowered the Nyx dev MySQL buffer-pool default through Compose configuration after the OOM evidence.

## Scenario Learnings

### Alpha: Normal Historical Backparse

- Clean `>1k/sec` remains real.
- Best current-source alpha evidence hit `1056.4/sec` average over 2M and `1092.8/sec` over the last half.
- The controller can reduce fetch below `112` and still hold rate.
- Alpha often rejects `35/6 -> 36/5`, so DB rebalance must remain reversible and scenario-specific.
- Longer runs reveal host pressure; rate alone is not enough.

### Beta: Bungie Latency

- Artificial `760ms` PGCR fetch latency makes the parser upstream-limited.
- The correct response is to add fetch capacity while DB queue stays empty.
- When latency is removed, high fetch must scale back down.
- Hot-lane DB pressure should block fetch-up even if total DB queue percentage is not high.

### Delta: Real-Time Edge

- At the front edge, scarcity is often intentional.
- Extra fetch workers can multiply not-ready/error pressure.
- The controller must recognize slow-roll/pause state and reduce fetch/retry workers toward the configured floor.
- Tiny slow-edge runs can validate cleanliness, but they do not always produce enough rate ticks to prove controller behavior.

### Gamma: Parser-Owned S3 Source

- Parser-owned S3 can feed much faster than Bungie.
- S3 abundance shifts the bottleneck toward DB/write lanes.
- Queue capacity matters because the controller needs enough observations during a fast staged run.
- Under S3 abundance, `35/6 -> 36/5` can be a good fixed-budget rebalance.
- That does not make `36/5` a universal alpha default.

## Important Incidents

### Transient Failed Rows

A fast run produced nonterminal failed rows and blocked rollup at the first incomplete cache window. Optimization stopped. The repair was to retry failed rows immediately at startup and keep retry policy conservative. A later repair proof cleared `raw_failed_pgcrs` to `0`, restored contiguous coverage, and let rollup advance.

### MySQL OOM During 5M Alpha

A 5M soak held roughly `1k/sec` before dev MySQL disappeared with an OOM. The parser shifted into repeated cache-stat failures with retry pressure. The response was:

- stop parser/Cortex process group;
- restart dev MySQL only;
- add a DB-unavailable hard brake;
- run normal parser recovery from DB truth;
- lower the dev MySQL buffer pool and prove a controlled 2M soak.

This was a host-capacity finding, not a parser correctness failure.

### Cache High-Water / Rollup Head

Cache high-water pressure could pause normal work while the rollup head was missing a normal uncommitted raw PGCR. That could block rollup progress. The fix was not a repair mode: allow normal parser work for raw gaps in the earliest rollup window under cache pressure, then continue to validate.

### Fetch Count Fixation

Owner feedback correctly identified that even calling `112` a seed gave it too much gravity. Source and tests were cleaned so `112` is not a policy concept. Live evidence now shows healthy runs can:

- climb through `112`;
- settle at `112` for a measured pressure shape;
- reduce below `112` to `96`, `88`, or lower;
- reduce to floor `4` in slow-edge mode;
- probe too low and recover.

## Current Operating Principles

- Prefer learned priors for production-style long runs.
- Use deliberately slow starts only when proving recovery behavior.
- Batch `128` is the current keeper ceiling, not a universal law.
- Fetch worker count must remain a measured control variable.
- DB rebalancing should be fixed-budget first: move one worker at a time, then accept or roll back.
- Rollup high-water is a safety brake, not the primary control loop.
- S3 source tests need enough queue capacity for controller observability.
- Stop/drain behavior is part of the autonomy contract.
- Normal parser restart from DB truth is the only recovery model.

## Validation Gates To Keep

- `dev/scripts/d1-milestone-status.sh`.
- D2 protected Redis counts.
- Raw coverage contiguous by count.
- `raw_failed_pgcrs=0`.
- S3 ledger/readback verification.
- Cache tail equals normal partial batch only.
- Datadog visibility.
- Host Redis health.
- MySQL/host memory and CPU pressure.
- No active parser/Cortex process after bounded runs.
- Hard-kill/restart recovery evidence before any unattended long run.

## Current Open Work

- Package the large dirty worktree into small reviewable commits.
- Continue longer alpha soaks with host pressure in the decision loop.
- Keep testing beta recovery so high fetch does not remain sticky.
- Tune delta production cadence and slow-edge fetch/batch policy.
- Continue gamma DB-lane tuning under S3 abundance.
- Decide whether rollup worker count should move from configurable to active autotuned.
- Add more durable controller learning/persistence only after the in-process behavior is calm and simple.

## Source Pointers

- Active tracker: [[Projects/Warmind/D1 parser validation and autotuning scenarios - 2026-05-24|D1 parser validation and autotuning scenarios]]
- Repo-local lessons: `/Users/Shared/projects/warmind-speculative/docs/specs/d1-autoscaling-lessons.md`
- Main D1 parser note: [[Projects/Warmind/D1 parser|D1 parser]]


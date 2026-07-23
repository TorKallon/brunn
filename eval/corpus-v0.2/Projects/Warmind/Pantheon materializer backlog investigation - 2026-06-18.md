Created: 2026-06-18
Updated: 2026-06-18
Status: investigation note
Related: [[Projects/Warmind/Warmind|Warmind]], [[Projects/Warmind/D1 parser|D1 parser]], [[Projects/Charlemagne/Logbook|Logbook]], [[Projects/Charlemagne/Research/Logbook Pantheon 2.0 research 2026-06-09|Logbook Pantheon 2.0 research]], [[Projects/Charlemagne/Research/Logbook Pantheon difficulty and Custom scoring research 2026-06-10|Pantheon difficulty and Custom scoring research]]

# Pantheon materializer backlog investigation - 2026-06-18

## Summary

The production Pantheon materializer is not failing. It is draining cleanly, but the work shape is serial and slow enough that `pantheon_pgcr_materialize` jobs can appear stacked even while rhea has spare CPU.

## Datadog evidence

Window: 2026-06-18 17:40-19:40 UTC.

- `charlemagne.sweeperbot.jobstart{jobname:pantheon_pgcr_materialize}`: about 70 starts in two hours.
- `charlemagne.sweeperbot.jobdone{jobname:pantheon_pgcr_materialize}`: about 70 completions in the same window.
- `charlemagne.pantheon.backfill.claimed`: about 7.1k rows claimed.
- `charlemagne.pantheon.backfill.materialize{status:done}`: about 7.1k rows completed.
- Materialization source/status observed as `source:s3,status:done`.
- `charlemagne.sweeper.jobduration.avg{jobname:pantheon_pgcr_materialize}`: roughly 98-107 seconds per job.
- Host rhea had spare CPU over the same window, roughly 38% idle average.
- MySQL `queries_queued` was 0, but row-lock wait/time counters and parser DB queue depth were nonzero/spiky.

## Code shape

- `sweeperbot/pantheon_backfill.go`: default materializer batch is 100 rows.
- The materializer claims one row at a time, applies it, and loops until the 100-row batch limit. If it hits the limit, it re-enqueues itself.
- `sweeperbot/sweeperbot.go`: `pantheon_pgcr_materialize` has `MaxConcurrency: 1` and is also periodically enqueued every 5 seconds.
- Parser successful commits can also wake the materializer after queuing Pantheon 2.0 rows.
- Applying a row writes `raw_pgcrs_pantheon`, updates `summary_raids_overall`, and marks the ledger row done in one transaction.

## Interpretation

The queue stack is scheduler/concurrency shape, not lack of host capacity:

- Each job takes about 100 seconds to process its 100-row batch.
- The periodic scheduler wakes the same job every 5 seconds.
- `MaxConcurrency: 1` allows only one materializer job to execute at a time.
- While one execution is running, more job entries can accumulate.
- Row throughput is about one row per second because the job is serial and each row performs DB summary writes.

Likely fixes to evaluate:

- Stop periodic enqueue from adding unbounded duplicate materializer jobs; prefer unique enqueue or a coarser cadence.
- Keep the self-reenqueue-on-full-batch path as the primary drain loop while backlog exists.
- Add a direct backlog metric for `pantheon_pgcr_backfill` status counts so queue depth is separated from row backlog.
- Only increase concurrency after confirming `summary_raids_overall` update contention is acceptable.

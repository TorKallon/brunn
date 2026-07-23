Created: 2026-06-18
Status: active incident note

## Summary

Warmind production saw large Datadog storms of MySQL `Error 1205: Lock wait timeout exceeded` from `warmind-parser` while D2 `error_reparse` was draining a large failed-PGCR backlog in `scaled` mode.

Current interpretation:

- Normal parser operation already had a low background rate of `UpdateWeaponMetaTX weekly` lock waits.
- The storm threshold lined up with `error_reparse` entering `scaled` mode on 2026-06-16.
- Error reparse is isolated at the fetch-queue layer, but after fetch it uses the same DB lane queues, parser transaction body, and MySQL summary rows as normal real-time parse.
- This can make real-time normal parse look chunky because DB workers wait on hot MySQL locks and completions arrive in bursts.

## Decision

Current plan:

1. Let scaled `error_reparse` finish the large backlog.
2. Return `warmind-parser.d2.error-reparse-mode` to `opportunistic`.
3. Recheck whether residual `UpdateWeaponMetaTX weekly` lock waits remain a real normal-parser problem.

Do not attempt another automated weapon-meta-outbox or post-commit redesign right now. Prior attempts at that sort of change failed badly enough that it should wait for a manual, human-owned pass.

## Evidence

Datadog counts gathered 2026-06-18:

| Window | Parser lock-wait logs | Explicit `ERROR_REPARSE` wrappers | Retry wrapper logs | Low-level `UpdateWeaponMetaTX weekly` logs |
|---|---:|---:|---:|---:|
| 2026-06-15 00:00-24:00 UTC | 295 | 115 | 7 | 145 |
| 2026-06-16 00:00-14:34:25 UTC | 142 | 0 | 17 | 125 |
| 2026-06-16 14:34:25-2026-06-17 04:30 UTC | 22,409 | 9,167 | 1,461 | 11,780 |

First observed `D2_PARSER_RATE` sample with `errorReparseMode: scaled`:

- `2026-06-16T14:34:25Z`
- `errorReparseEffectiveMaxFetchRate: 150`
- `errorReparseQueue` filled to about `607-608` within seconds.
- `errorReparseDBInFlight` climbed from `0` into the 40s within minutes.
- During the largest waves, `errorReparseDBInFlight` frequently sat around `71-72`.

Important limitation: the low-level `MYSQL ERROR in UpdateWeaponMetaTX weekly` line does not carry job type. Error-reparse attribution comes from the higher-level `ERROR_REPARSE` and retry-wrapper logs that wrap the same failed parser error path.

## Repo documentation

Detailed repo note:

- `/Users/Shared/projects/warmind/docs/agents/error-reparse-lock-wait-storm-2026-06-18.md`

Updated triage note:

- `/Users/Shared/projects/warmind/docs/agents/datadog-error-triage-2026-06-16.md`

## Datadog queries

Parser lock waits:

```text
service:warmind-parser ("Lock wait timeout exceeded" OR "Error 1205")
```

Explicit error-reparse lock waits:

```text
service:warmind-parser "ERROR_REPARSE:" "Error 1205"
```

Parser rate logs:

```text
service:warmind-parser "D2_PARSER_RATE"
```

## Watch while waiting

- `charlemagne.warmind.parser.error_reparse_queued`
- `charlemagne.warmind.parser.error_reparse_db_inflight`
- `charlemagne.warmind.parser.error_reparse_fetch_queue_depth`
- `charlemagne.parser.db_queue_wait.avg`
- `charlemagne.parser.db_process.avg`
- `charlemagne.parser.txtime.avg`
- `charlemagne.warmind.parser.normal_frontier_hold`
- Parser `Error 1205` grouped by message and hour.

## Related

- [[Projects/Warmind/Pantheon materializer backlog investigation - 2026-06-18|Pantheon materializer backlog investigation]]
- [[Projects/Warmind/Bungie API throttle incident - 2026-06-09|Bungie API throttle incident - 2026-06-09]]
- [[Projects/Warmind/Warmind|Warmind]]

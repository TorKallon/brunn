Created: 2026-06-12
Updated: 2026-06-14
Status: retired pending deploy/restart and Redis cleanup
Related: [[Projects/Warmind/Warmind|Warmind]], [[Projects/Charlemagne/Charlemagne|Charlemagne]], [[Projects/Warmind/D1 parser|D1 parser]], [[Projects/Warmind/Session tracking Nyx soak progress - 2026-06-08|Session tracking Nyx soak progress]], [[Projects/Warmind/D2 first played production integrity plan - 2026-06-08|D2 first played production integrity plan]]

# D2 first played backfill retired - 2026-06-12

## Summary

The `update_d2_first_played` Cortex job was retired after it created sustained production `sweeperbot` queue pressure.

## Local change
- `sweeperbot.d2FirstPlayedBackfillEnabled()` now always returns `false`, so `warmind-cortex` will not register the job and profile updates will not enqueue it after the next deploy/restart.
- `sweeperbot/profile_play_bounds_test.go` asserts the legacy `features.d2-first-played-backfill` config flag cannot re-enable the job.
- Verification passed on Nyx: `go test ./sweeperbot -run 'TestD2FirstPlayed' -count=1`, `go test ./sweeperbot -count=1`, `go build ./cmd/warmind-cortex`, and `git diff --check -- sweeperbot/sweeperbot.go sweeperbot/profile_play_bounds_test.go`.

## Production evidence
- Datadog read at about 2026-06-12 09:14 PDT for host `rhea` reported `charlemagne.sweepergo.qlen{task:update_d2_first_played}` latest/max around `669455` over the prior 30 minutes.
- Same read showed `skill_rating_pgcr` around `11448`, `session_pgcr_seen` latest `0` / max `62`, `session_sessionize_due` latest `0` / max `14`, and `session_finalize_due` `0`.

## Access caveat
- This Nyx session did not have a usable SSH key for `charlemagne@rhea.warmind.io`, `charlemagne@44.239.88.159`, `ubuntu@rhea.warmind.io`, `ec2-user@rhea.warmind.io`, or the `rhea-c` alias. Production Redis cleanup was not executed from Nyx.

## Operator cleanup shape

Once on Rhea after the disabled Cortex binary is deployed/restarted or the live config is turned off and Cortex restarted:

```bash
set -euo pipefail

job=update_d2_first_played
r='redis-cli -p 6380'

echo "before queue_len=$($r LLEN "sweeperbot:jobs:$job") retry=$($r ZCARD sweeperbot:retry) dead=$($r ZCARD sweeperbot:dead) scheduled=$($r ZCARD sweeperbot:scheduled)"

$r DEL "sweeperbot:jobs:$job" \
  "sweeperbot:jobs:$job:max_concurrency" \
  "sweeperbot:jobs:$job:lock" \
  "sweeperbot:jobs:$job:lock_info" \
  "sweeperbot:jobs:$job:paused"
$r SREM sweeperbot:known_jobs "$job"

$r EVAL "local removed=0; for _,key in ipairs(KEYS) do local vals=redis.call('ZRANGE', key, 0, -1); for _,v in ipairs(vals) do local ok,j=pcall(cjson.decode, v); if ok and j['name']==ARGV[1] then redis.call('ZREM', key, v); removed=removed+1 end end end return removed" 3 sweeperbot:scheduled sweeperbot:retry sweeperbot:dead "$job"

$r --scan --pattern "sweeperbot:unique:$job:*" | xargs -r redis-cli -p 6380 DEL
$r --scan --pattern "sweeperbot:jobs:$job:*:inprogress" | xargs -r redis-cli -p 6380 DEL

echo "after queue_len=$($r LLEN "sweeperbot:jobs:$job") known=$($r SISMEMBER sweeperbot:known_jobs "$job")"
```

Do not run the cleanup before stopping the old enqueue path; otherwise profile leaderboard updates can immediately recreate jobs.

Created: 2026-06-09
Updated: 2026-06-09 16:30 PDT
Status: investigation
Related: [[Projects/Warmind/Warmind|Warmind]], [[Projects/Charlemagne/Charlemagne|Charlemagne]], [[Projects/Warmind/D1 parser|D1 parser]]

# Bungie API throttle incident - 2026-06-09

## Summary

Warmind production on `rhea` hit a new `warmind-cortex` Bungie API throttle wave on 2026-06-09. Datadog logs show the current wave is dominated by Bungie `ErrorCode 55` / `PerApplicationAnonymousThrottleExceeded`, not a database or Discord failure.

Separate earlier noise in the same 6-hour window came from Bungie `ErrorCode 5` maintenance.

## Datadog evidence

- Last 1 hour at investigation time:
  - `GetD2ActivityHistory: `: about 113,000 cortex error logs.
  - `CLAN_MEMBER_SYNC: sync clan members, GetD2ClanMembers`: about 7,600 cortex error logs.
  - Main error message: `Bungie API Error #55: This application has made too many unauthenticated requests. Try again later.`
- Last 5 minutes at investigation time:
  - `GetD2ActivityHistory` #55: about 25,300 logs.
  - Clan-member #55: about 4,000 logs.
- Last 6 hours:
  - Earlier Bungie maintenance wave: about 705,000 `ErrorCode 5` logs, mostly around 05:57-06:57 PDT.
  - Current throttle wave restarted around 10:27 PDT and was still active around 11:27 PDT.
- High-volume related metrics in the last hour:
  - Bungie endpoint calls: `stats`, `pgcr`, `profile`, `user`, `clan-rewards`, `members`.
  - Sweeper jobs: `global_profile_leaderboards`, `update_clan_info`, `session_pgcr_seen`, `update_d2_first_played`, `update_charl_pve`, `sync_clan_members`.

## Code observations

- `bungie/client.go` sends all non-token public Bungie calls through `GetApiKey()` and has no global limiter for public non-PGCR calls.
- `bungie/errors.go` already classifies `BErrorPerApplicationAnonymousThrottle = 55` as rate-limited.
- `sweeperbot/profile_play_bounds.go` can page activity history up to 2,000 pages of 250 activities for the account and for characters.
- The first-played worker was recently introduced as `update_d2_first_played`; prior memory records the production-oriented concurrency being raised to `100`. The current local checkout contains uncommitted mitigation to make it configurable with default `10`.
- `sweeperbot/sweeperbot.go` registers `sync_clan_members`, pending member checks, and invited member checks at `MaxConcurrency: 15`.
- `sweeperbot/charlemagne.go` enqueues one `sync_clan_members` job per synced clan.

## Proposed response

Immediate mitigation:
- Reduce or pause `update_d2_first_played` and clan-member sync consumers, then watch Datadog until Bungie #55 logs fall near zero.
- Deploy the local first-played concurrency mitigation or a stricter emergency variant with concurrency `1`.
- Temporarily slow or disable `features.clan-sync`, `check_pending_clan_members`, and `check_invited_clan_members` if production config allows it without a code deploy.

Code fixes:
- Add a dedicated feature flag for D2 first-played backfill; do not rely only on `features.cortex-bungie-api`.
- Add a global public Bungie API rate limiter/backoff path in `bungie/client.go`, separate from parser PGCR-specific limits.
- On Bungie `RateLimited()` or `Maintenance()`, stop logging one error per attempted page/member request; emit sampled logs plus structured metrics by endpoint/code.
- Lower clan sync concurrency from hard-coded `15` to config, with emergency defaults around `1-3`.
- Prefer offline PGCR archive/cache backfill for historical first-played bounds; reserve live activity-history paging for tails and misses.

## References

- `bungie/client.go`
- `bungie/errors.go`
- `bungie/api.go`
- `sweeperbot/profile_play_bounds.go`
- `sweeperbot/sweeperbot.go`
- `sweeperbot/charlemagne.go`

## Follow-up recheck

Rechecked at about 2026-06-09 11:37 PDT. The current errors were still mostly Bungie `ErrorCode 55`.

Last 5 minutes:
- `GetD2ActivityHistory`: about 2,024 error logs.
- `CLAN_MEMBER_SYNC: sync clan members, GetD2ClanMembers`: 4 error logs.
- Error messages: about 1,892 Bungie `#55` logs and 136 Bungie `#1665` private-profile logs.

Last 15 minutes:
- About 55,498 Bungie `#55` logs.

Safety review:
- D2 first-played handling is safe for `#55`; rate-limit errors are not classified as expected/no-history and therefore do not mark `firstPlayedD2AttemptedAt` or write `firstPlayedD2`.
- Initial clan member list failure is safe; `syncD2ClanMembers` returns before reconciliation writes when `GetD2ClanMembers` fails.
- Later clan reconciliation had a corruption-risk bug: two `getMaybeNewClan` calls ignored `err`, so a Bungie `#55` could continue with `newClanId == 0` and write an unknown clan state. Datadog showed this later lookup path was live, with 57 `CLAN_MEMBER_SYNC: getD2Clan` #55 warnings in the prior hour.
- Patched `sweeperbot/charlemagne.go` to return on those two lookup errors before `SetProfileClanId` / `UpsertClanMember`.
- Verified with `go test ./sweeperbot -count=1`.

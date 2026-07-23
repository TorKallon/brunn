Created: 2026-06-24 07:10 PDT
Updated: 2026-06-24 PDT
Status: implemented locally
Related: [[Projects/Warmind/Warmind]], [[Projects/Warmind/D1 parser]], [[Projects/Charlemagne/Charlemagne]]

# Mode 0 Override Moon investigation - 2026-06-24

## Summary

Production `warmind-parser` showed new D2 permanent failures for:

- `mode_0_error--dhash:3480889442`
- `mode_0_error--dhash:2301670494`

Both hashes are Splicer Override Moon activities whose PGCRs currently arrive with `ActivityDetails.Mode = 0` and no activity mode list.

## Datadog Evidence

Metric: `sum:charlemagne.warmind.permafail{*} by {reason}.as_count()`

Last 24h as of 2026-06-24 07:05 PDT:

- `mode_0_error--dhash:3480889442`: 6859
- `mode_0_error--dhash:2301670494`: 760
- `playercountmismatch--player_count_in_pgcr_does_not_match_actual_number_of_players.`: 11

Last 6h:

- `mode_0_error--dhash:3480889442`: 1188
- `mode_0_error--dhash:2301670494`: 136
- `playercountmismatch--player_count_in_pgcr_does_not_match_actual_number_of_players.`: 1

Raw parser logs searched:

- `JOB_PERMANENT_FAILURE service:warmind-parser`
- `service:warmind-parser status:error "mode: 0 for PGCR"`

Permanent failure logs exposed `custom.errMessage` and `custom.pgcrId`.

## Manifest Evidence

Bungie manifest lookups using the repo dev Bungie API key:

| directorActivityHash | Name | PvP | Matchmade | Max players | Direct mode |
| --- | --- | --- | --- | --- | --- |
| `3480889442` | `Override: The Moon: Matchmade` | false | true | 6 | none |
| `2301670494` | `Override: The Moon: Customize` | false | false | 6 | none |

Both share description: "Use teamwork and your Splicer skills to force your way into the Vex network and crash it from the inside."

## PGCR Evidence

`3480889442`

- First permanent-failure sample: 15 matching PGCRs.
- Broader recent mode-zero sample: 111 matching PGCRs from the 120 most recent `mode: 0 for PGCR` logs.
- Kills/deaths fields present in 111/111.
- Nonzero kills in 111/111; nonzero deaths in 110/111.
- Max entries: 8; max distinct players: 8.
- Duration: min 134s, max 827s, average 654.41s.
- Modes seen: `[0]`.
- Activity modes seen: none.
- References seen: `[3480889442]`.

Representative PGCR ids:

`16965835826`, `16965834571`, `16965833613`, `16965833644`, `16965832876`, `16965831924`, `16965830670`, `16965830182`, `16965828755`, `16965828567`, `16965828538`, `16965827362`.

`2301670494`

- First permanent-failure sample: 4 matching PGCRs.
- Broader recent mode-zero sample: 9 matching PGCRs from the 120 most recent `mode: 0 for PGCR` logs.
- Kills/deaths fields present in 9/9.
- Nonzero kills in 9/9; nonzero deaths in 9/9.
- Max entries: 6; max distinct players: 6.
- Duration: min 313s, max 1737s, average 1095.67s.
- Modes seen: `[0]`.
- Activity modes seen: none.
- References seen: `[2301670494]`.

PGCR ids:

`16965836784`, `16965823352`, `16965801798`, `16965796799`, `16965791614`, `16965764465`, `16965763856`, `16965761588`, `16965746543`.

## Decision

Classify both as `ModeOffensive` plus `ModeAllPve`, matching existing Override workaround family:

- `1792588602`: `Override: Europa: Matchmade`
- `1307933814`: `Override: Europa: Customize`
- `3343628502`: `Override: Last City: Matchmade`
- `883763122`: `Override: Last City: Customize`

Patch:

- Add `3480889442` and `2301670494` to `modeZeroOffensiveActivities` in `bungie/workarounds.go`.
- Add table-driven tests in `bungie/workarounds_test.go`.

## Validation

Commands run from `/Users/shared/projects/warmind`:

```sh
go test -count=1 ./bungie
go test -count=1 ./parser ./nexusdb ./memcache
git diff --check -- bungie/workarounds.go bungie/workarounds_test.go
```

All passed.

## Follow-Up

After deploy, watch `charlemagne.warmind.permafail` grouped by `reason` for the two hashes. If production error-reparse is used to drain `raw_failed_pgcrs`, remember the current D2 error-reparse path fetches Bungie fresh and deletes `raw_failed_pgcrs` on success, but it does not replace old `raw_pgcrs` mode-255 coverage markers or already-uploaded S3 batch slots without a separate repair path.

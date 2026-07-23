# Charlemagne alerts diagnostic handoff - 2026-06-24

Updated: 2026-06-24 16:30 PDT
Related: [[Projects/Charlemagne/Charlemagne|Charlemagne]], [[Projects/Warmind/Warmind|Warmind]], [[Projects/Warmind/D1 parser|D1 parser]], [[Active projects]]

Source thread: https://canary.discord.com/channels/291026626647425025/1518950167368171540

Guild: `291026626647425025`
Forum/channel: `ask-help-here` / `1019670742767505438`
Thread: `I'm the only person that seems to be randomly ignored (ALERTS)` / `1518950167368171540`
Tag: `Notifications/Alerts` / `1019681481032990830`
Reporter: Discord `432542706242420750`, username/display `uksflamez` / `UKsFLAMEZ`
Known Bungie name from screenshots: `UKsFLAMEZ#7659` / `UKSFLAMEZ#7659` (case varies in screenshots/OCR)

## Executive summary

A user reports Charlemagne/Warmind achievement alerts are intermittent only for them. Server-level alert configuration appears enabled, other members continue to receive the same classes of alerts in the same achievement channel, and re-registering did not resolve it.

Reported missed alert classes:

- Pinnacle / Quest Exotic / Ritual weapon alerts
- Title alerts, including Gilded Conqueror
- Guardian Rank alerts, specifically reaching rank 11

Important code-path suspicion: this does not look like a generic Discord send outage. The likely failure is earlier, in per-profile state transition detection: collectible ownership rows, seal/gilded seal bitmasks, or guardian-rank stats already reflect the new state before the alert trigger observes a false-to-true / old-to-new transition.

## Thread chronology

All thread timestamps below are Discord API UTC timestamps unless noted.

- 2026-06-23 12:05:40, reporter opens the thread:
  - "Everyone else in the server it works fine for, just very hit and miss for me sometimes posts but most of the time it doesn't."
  - "been happening for a while tbh just seems like it's got worse"
  - Attachment `1518950167687069736` shows subscription/config UI:
    - Pinnacle Alerts enabled to `#achievements`
    - Flawless Alerts enabled to `#achievements`
    - Title Alerts enabled to `#achievements`
    - Guardian Rank Alerts enabled to `#achievements`
- 2026-06-23 12:06:51:
  - "I have also Re-registered, that didnt help"
- 2026-06-23 14:07:05, Hanxa asks for recent examples:
  - Asks whether all these alerts are hit-or-miss.
  - Asks for title and pinnacle examples.
  - Notes Flawless alerts are only for first flawless.
- 2026-06-23 15:18:46:
  - Reporter says: "new land beyond i got same day, still waiting for mine to post lol, ive hit rank 11, that didnt come either.. gilded conqueror that didnt come, literally everyone else seem to come through fine"
  - Attachments show other users receiving weapon/armor alerts around the same period.
- 2026-06-23 15:19:48:
  - Reporter says "the last one i got was" and attaches `1518999021815992482`.
  - Screenshot shows a Title Alert for `UKsFLAMEZ#7659`: `Undertaker`, earned `05/05/2026 20:01`.
- 2026-06-23 15:21:56:
  - Reporter says "apparently the last exotic i got was" and attaches `1518999559999721513`.
  - Screenshot shows Armor Alert for `UKsFLAMEZ#7659`: `Arbor Warden`, earned `20/03/2026 23:09`.
- 2026-06-23 15:26:52:
  - Reporter adds: "some people also get posts twice (they got it 2 seperate times)"
- 2026-06-23 15:27:27, Hanxa:
  - "Seeing that here as well. I'm gonna bump this up for ideas."
  - Says last alert seen was about a month ago, only a couple two months ago, and three months ago seemed fine.
- 2026-06-23 15:28:19:
  - Reporter says they had not played for 3 years, came back in the last few months of last season.
  - Attachment `1519001166292324412` shows duplicate-looking alert evidence for another user.

## Screenshot evidence

Attachment `1518950167687069736`, opening post:

- Config page shows all four relevant alert types routed to `#achievements`.
- This supports "server is configured" but does not prove whether individual events were generated.

Attachment group on message `1518998763895787580`, reporter's "new land beyond" message:

- `1518998763463643337`:
  - Warmind posted two Weapon Alerts for `naitohoku` / Bungie `Nighthawk(火影)#0313`.
  - `New Land Beyond`, Edge of Fate pre-order exotic, earned `10/06/2026 08:56`.
  - `Resounding`, Ritual Activity Legendary from Episode 3: Heresy, earned `10/06/2026 08:56`.
  - Reporter says they earned New Land Beyond the same day but their own alert did not post.
- `1518998763170037810`:
  - Other users receiving alerts:
    - `crenshaw.` / `Crenshaw#9264`: `Praxis Vestment`, `19/06/2026 22:29`.
    - `drax9466` / `DraX#2642`: `New Malpais`, `19/06/2026 22:29`.
    - `jtfromit` or similar / `EsoTerrestrial#4749`: `Fallen Sunstar`, `Cadmus Ridge Lancecap`, `Speedloader Slacks`, all `20/06/2026 08:10`.
- `1518998762696216838`:
  - More other-user alerts:
    - `jjtfromit` / `EsoTerrestrial#4749`: `Chivalric Fire`, `20/06/2026 08:10`.
    - `naitohoku` / `Nighthawk(火影)#0313`: `Deimosuffusion`, `20/06/2026 18:29`.
    - `warmachrox` / `WarMachine#0382`: `Eunoia`, `21/06/2026 14:26`.
    - `crenshaw.` / `Crenshaw#9264`: `Still Hunt`, "Yesterday at 16:29".
    - `crenshaw.` / `Crenshaw#9264`: `Wish-Keeper`, "Yesterday at 19:39".

Attachment `1518999021815992482`, reporter's "last one I got":

- Warmind Title Alert for `uksflamez` / `UKsFLAMEZ#7659`.
- Title: `Undertaker`.
- Earned `05/05/2026 20:01`.

Attachment `1518999559999721513`, reporter's "last exotic I got":

- Warmind Armor Alert for `uksflamez` / `UKsFLAMEZ#7659`.
- Item: `Arbor Warden`.
- Source: Lost Sector Exotic from Season of the Deep.
- Earned `20/03/2026 23:09`.

Attachment `1519001166292324412`, reporter's break/return context:

- Shows two Warmind Weapon Alerts for `crenshaw.` / `Crenshaw#9264`, both for `Barrow-Dyad`.
- First visible earned timestamp: `11/06/2026 20:51`.
- Second visible earned timestamp: `11/06/2026 22:11`.
- Supports the separate duplicate-alert complaint.

## Code pointers

Start in `/users/shared/projects/warmind`; the older Python repo `/users/shared/projects/charlemagne` is probably only useful for historical comparison.

Relevant preference keys:

- `charlemagne/guildprefs.go`
  - `GPrefTypeFlawlessAlerts = "FlawlessNotifications"`
  - `GPrefTypePinnacleAlerts = "pinnacleCongrats"`
  - `GPrefTypeSealAlerts = "SealNotifications"`
  - `GPrefTypeGuardianRankAlerts = "guardianRankNotifications"`

Pinnacle / exotic / ritual weapon and armor alerts:

- `sweeperbot/charlemagne.go`
  - Profile update path calls `updatePinnacleStatus` only if `bungieProf.ProfileRecords.Data.Records != nil`.
- `sweeperbot/helpers.go`
  - `updatePinnacleStatus(bungieProf, ubp)`:
    - skips private profiles.
    - loads `charlemagne.GetAllPinnacleStatus(ubp.MembershipID)`.
    - for a missing `pinnacle_status` row, it silently seeds `owned = isOwned`.
    - only sends when an existing row changes from `owned=false` to `owned=true`.
    - sends before updating `pinnacle_status`.
- `charlemagne/pinnacles.go`
  - `pinnacle_status` has only `membershipId`, `pinnacleId`, `owned`.
  - There are no timestamps in the struct, so historical diagnosis may need DB binlog/logs/SNS logs if table schema has no audit columns.
- `notifications/pinnacles.go`
  - `SendPinnacleCongratsNotification` sends SNS topic `pinnacleCongrats` with `TargetMemID`.
  - Formatter title becomes `Weapon Alert` or `Armor Alert`.

Title and gilded title alerts:

- `sweeperbot/helpers.go`
  - `upsertSealsAndFire`:
    - computes seal bitmask diff.
    - does not send if the DB row is new (`!isNew` guard).
    - sends `notifications.SendSealEarnedNotifications(..., gilded=false)` only on non-new rows with a positive diff.
  - `upsertGildedSealsAndFire`:
    - same pattern for current-season gilded rows.
    - sends only when row is not new and a new gilded bit is observed.
- `notifications/seals.go`
  - Sends SNS topic `SealNotifications`.
  - Formatter uses `Title Alert` or `Gilded Title Alert`.

Guardian Rank alerts:

- `sweeperbot/stats.go`
  - `SaveStats` enqueues `notification_guardian_rank` only if `updatedStats[StatTypeGuardianRank]` and `oldValue > 0 && newValue > oldValue`.
  - `StatTypeGuardianRank = 607`.
- `discord/dwork/notifications.go`
  - `sendGuardianRankAlert` calls `notifications.SendGuardianRankNotification`.
- `notifications/guardianrank.go`
  - Sends SNS topic `guardianRankNotifications`.

Duplicate alert angle:

- `updatePinnacleStatus` sends the notification before persisting the new `pinnacle_status` value.
- If two profile update workers process the same membership concurrently, both could read `owned=false`, both send, then both set true.
- There may also be duplicate SNS/fanout delivery downstream, but the collectible transition path has an obvious race window.

## Working hypotheses

1. Most likely for missed personal alerts: state was seeded or already advanced before the alert trigger saw a transition.
   - Pinnacle path: no row means "seed silently", not "alert".
   - Seal/gilded path: new row means "upsert silently", not "alert".
   - Guardian Rank path: no old value or old value already equal to 11 means no alert.
   - This fits a returning/re-registered user and also fits a user whose profile was private or not actively scanned at the exact moment of acquisition.

2. Possible profile polling / eligibility issue:
   - The user says others in the same server are fine, so check whether `UKsFLAMEZ#7659` is in the active profile update pool and whether profile updates occurred around 2026-06-10 through 2026-06-23.
   - If their profile was skipped due to privacy, token/registration issues, missing `ProfileRecords`, or stale registration mapping, the later catch-up could silently seed state.

3. Possible manifest/list timing issue for new Edge of Fate / Ash & Iron items:
   - If `New Land Beyond` or new titles were not in `bungie.PinnacleList` / seal maps when this user earned them, the first scan after list update may have treated them as already owned and seeded state silently.
   - Counterpoint: another user got a New Land Beyond alert on 2026-06-10, so the list existed at least for that observed alert.

4. Separate duplicate issue likely exists:
   - The Barrow-Dyad screenshot and reporter comment suggest duplicate posts for some users.
   - Start by checking whether duplicate SNS messages were emitted or one SNS message fanned out twice.
   - If two SNS messages exist, focus on transition detection race / missing idempotency.
   - If one SNS message exists, focus on Discord fanout retry/idempotency.

## Suggested diagnostic steps

1. Resolve reporter's production identifiers.
   - Discord ID: `432542706242420750`.
   - Bungie name: `UKsFLAMEZ#7659`.
   - Need production `user_id`, `user_bungie_profiles.id`, `membership_id`, and `membership_type`.
   - Do not rely on Bungie name case; screenshots/OCR show both `UKsFLAMEZ#7659` and `UKSFLAMEZ#7659` variants.

2. Confirm server/channel prefs for guild `291026626647425025`.
   - Verify `pinnacleCongrats`, `SealNotifications`, `guardianRankNotifications`, and `FlawlessNotifications` point at the intended achievement channel.
   - This probably passes because other users are receiving alerts, but it rules out user-specific/channel-specific fanout filters.

3. Check reporter state tables.
   - `charlemagne.pinnacle_status` for reporter membership:
     - Count total rows.
     - Check rows for `New Land Beyond`, `Arbor Warden`, and other missed/recent item hashes once hashes are resolved.
     - Determine whether missed items are already `owned=true`.
   - `nexus.seals` for reporter membership:
     - Does the row exist?
     - Does it already include Conqueror / relevant title bit?
   - `nexus.seals_gilded` for reporter membership and current season:
     - Does the row exist?
     - Does it already include Gilded Conqueror?
   - `charlemagne.stats` for reporter profile and `stat_type_id=607`:
     - Was old value already 11 or missing?

4. Check profile update history/logs.
   - Search around:
     - 2026-06-10 08:56 screenshot time for New Land Beyond comparison.
     - 2026-06-23 report time.
     - 2026-05-05 20:01 Undertaker alert.
     - 2026-03-20 23:09 Arbor Warden alert.
   - Look for:
     - `UpdatePinnacleStatus`
     - `Send Pinnacle Congrats Error`
     - `User <membershipID> - <BungieName> has acquired`
     - `UPSERT_SEALS`
     - `UPSERT_GILDED_SEALS`
     - `Firing guardian rank up`
     - `notification_guardian_rank`
     - profile privacy / `ProfileRecords` nil skips.

5. Compare with a known-good user from screenshots.
   - Good comparison candidates:
     - `Nighthawk(火影)#0313` for `New Land Beyond` on 2026-06-10.
     - `Crenshaw#9264` for `Praxis Vestment`, `Still Hunt`, `Wish-Keeper`, and duplicate `Barrow-Dyad`.
   - Compare their `pinnacle_status` row history/counts and profile update cadence against `UKsFLAMEZ#7659`.

6. Reproduce locally with a focused unit/integration test if possible.
   - Pinnacle:
     - Given no `pinnacle_status` row and `CheckGearIsEarned=true`, current behavior seeds without alert.
     - Given existing `owned=false` and `CheckGearIsEarned=true`, sends once and sets true.
     - Given two concurrent updates, verify whether two sends can happen.
   - Seals/gilded:
     - Given no row and new bitmask already contains title, current behavior upserts without alert.
     - Given existing row without bit and new row with bit, sends.
   - Guardian Rank:
     - Given no old stat or old stat 0 and new stat 11, current behavior does not send.
     - Given old stat 10 and new stat 11, sends.

## Potential fixes to consider

Do not blindly flip all "new row" paths to alert, because that may spam returning users, privacy flips, or mass manifest corrections. Safer options:

- Add targeted diagnostic logging first:
  - When seeding `pinnacle_status` with `owned=true`, log membership ID, item hash/name, reason, and current registration/profile freshness.
  - When skipping seal/gilded alerts due to `isNew`, log membership ID, decoded titles, season, and registration freshness.
  - When Guardian Rank old value is 0/missing and new value is high, log as a suppressed initial-seed alert.
- Add idempotency for send-before-state-update races:
  - Use a per-membership+alert-type+item key, conditional insert, or transaction before SNS send.
  - For pinnacle alerts, a table like `notification_dedupe(membershipId, alertType, subjectHash, seasonOrScope)` would be cleaner than relying on `pinnacle_status`.
- Consider a bounded grace/backfill rule for returning/re-registered users:
  - If a profile has previous successful alert history and is not a brand-new registration, treat some "seed owned=true" events as eligible for alert if acquisition time can be inferred confidently.
  - If acquisition time cannot be inferred, prefer surfacing a support diagnostic command over sending late alerts.
- Add a support/debug command or admin report:
  - For a Discord user or Bungie name, show last profile scan, privacy state, current stored pinnacle/seal/rank state, last emitted alert per type, and suppressed initial-seed counts.

## Questions for the reporter / support

- Exact achievement times for the missed `New Land Beyond`, Guardian Rank 11, and Gilded Conqueror events if they can provide them from Bungie/Destiny UI.
- Whether the user's Bungie profile was private at any point during the missed events.
- Whether re-registration changed their linked Bungie membership ID, platform, cross-save primary, or default profile.
- Whether the achievement channel had deleted messages around those times; screenshots imply missing posts, but audit/log confirmation would help.

## Short handoff for Codex

Investigate `UKsFLAMEZ#7659` / Discord `432542706242420750` in production. The issue is probably not channel prefs or Discord delivery, since other users in guild `291026626647425025` receive the same alerts. Check whether this user's profile state was already seeded/advanced before alert triggers observed deltas. Start with `pinnacle_status`, `nexus.seals`, `nexus.seals_gilded`, and `stats.stat_type_id=607`, then compare against known-good users `Nighthawk(火影)#0313` and `Crenshaw#9264`. Also check duplicate protection in `updatePinnacleStatus`, because it sends before updating ownership state and may permit duplicate SNS sends under concurrent profile updates.

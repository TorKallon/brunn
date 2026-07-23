Created: 2026-07-11
Updated: 2026-07-11

Related: [[Projects/RuptureOps/RuptureOps|RuptureOps]], [[Topics/StarRupture/StarRupture|StarRupture]], [[Topics/StarRupture/Game mechanics - Update 1|Game mechanics]], [[Topics/StarRupture/Player save - exploration and combat|Exploration and combat]], [[Topics/StarRupture/Food strategy - permanent low-chore system|Food strategy]]

# Rupture cycle timer - research and product design

## Decision

The first RuptureOps feature is a native iPhone **Watch Session** for the Ruptura cycle. It is not just a clock. Its primary job is to answer three questions from several feet away while the player is still looking at the game:

1. What is happening now?
2. What becomes dangerous next, and exactly when?
3. What is worth doing during the remaining window?

The main screen should therefore lead with a large countdown to the next meaningful event, not elapsed cycle time or a generic phase name. During post-wave recovery, the hero event is the expected return of ordinary Vermin. During normal ecology, the hero event is the next Ruptura or the beginning of its warning phase.

The wording must remain conservative. The post-wave period is a **lower-threat opportunity window**, not guaranteed safety. Triggered Geo Scanner, monolith, infection, Base Core, and scripted POI encounters can ignore the environmental cycle.

## Version and evidence boundary

This design is pinned to:

- current public game boundary: Early Access Update 1 through Hotfix `0.2.8`, Steam build `23761620`, released 2026-06-17;
- structured-source capture: SRDB `2.3.4`, captured 2026-07-11;
- community timer model: 54 minutes from local fire-wave impact to the next impact.

Creepy Jar has not published an official numerical phase formula. The 54-minute definition is strongly corroborated by the current `starrupture.tools` client and an independent open-source mod that reads the game runtime, but RuptureOps must store it as a versioned community/game-data model rather than an eternal official invariant.

Enemy-return events and gathering windows are separate versioned facts. Do not encode them as universal consequences of a phase boundary merely because they currently align.

## What `starrupture.tools` currently does

The live [Rupture Time Tracker](https://starrupture.tools/timer) is a manually anchored wall-clock phase ruler.

Its current source models:

| Site phase | Cycle position | Duration | Proportional width |
|---|---:|---:|---:|
| World burning | `0:00-0:30` | 30 seconds | 0.93% |
| World cooling | `0:30-1:30` | 60 seconds | 1.85% |
| World stabalizing | `1:30-11:30` | 10 minutes | 18.52% |
| World stable | `11:30-54:00` | 42 minutes 30 seconds | 78.70% |

The misspelling `stabalizing` is present in the live source. On a roughly 360-point phone content width, the first two segments are only about 3 and 7 points wide.

The page provides:

- a 48-pixel proportional cycle timeline with draggable playhead;
- four phase cards with duration, absolute start/end clock time, and relative start/end time;
- start, pause, resume, reset, and share-link controls;
- local persistence of anchor and paused state;
- a configurable one-shot pre-Ruptura web-audio warning;
- an optional tone at phase transitions.

Reset means “cycle zero now,” and the page advises resetting when the Ruptura hits. Sharing sends an absolute start timestamp, but not pause state or alert settings.

The useful parts to retain are drag-to-sync, phase-boundary shortcuts, absolute event times, pause/resume, a shareable anchor, and a whole-cycle overview.

The main product gaps are:

- no large, across-the-room countdown;
- no enemy-return event or warning;
- no opportunity guidance;
- no explicit unsynced or confidence state;
- no game/server connection or wave number;
- no native haptics, notifications, wake lock, Live Activity, or OS-grade alarm;
- phone layout puts stacked phase cards and controls ahead of a field-ready information hierarchy;
- short dangerous phases nearly disappear in the proportional timeline.

The live client bundle reviewed on 2026-07-11 matches the local SRDB capture. A fresh visual screenshot audit was not possible because no supported browser surface was available; the hierarchy above comes from the live RSC/client source and responsive classes.

## Player-meaningful timeline

RuptureOps should preserve the 54-minute engine model while exposing five player-meaningful states:

| State | Cycle position | Duration | Primary message |
|---|---:|---:|---|
| Fire wave | `0:00-0:30` | 0:30 | Remain sheltered |
| Cooling | `0:30-1:30` | 1:00 | Remain sheltered |
| Recovery / quiet window | `1:30-11:30` | 10:00 | Lower ambient threat; use the window |
| World active | `11:30-51:30` | about 40:00 | Ordinary enemies and ecology active |
| Ruptura warning | `51:30-54:00` | about 2:30 | Return, bank loot, and shelter |

The site rolls the final warning into its 42:30 Stable block. Current game-runtime tooling exposes `Warning` separately. Community stopwatch reports broadly agree on about two minutes of white warning, 15 seconds of yellow warning, and a final roughly 15-second countdown, but those internal warning boundaries still need a live Hotfix 0.2.8 validation run.

Manual synchronization is anchored at `0:00` when the fire wall reaches the player, not when the player exits shelter or when recovery begins. The moving wave may make the global-to-local offset location-dependent.

## Main monitor design

### Core hierarchy

The portrait Watch Session has five layers, all visible without scrolling:

1. **Sync and capability line**
   - `Manual sync - predicted - 21m ago`
   - sound, haptic, notification, loud-alarm, and screen-awake status
   - a clear degraded or unsynced state
2. **Hero state and countdown**
   - large semantic state such as `QUIET WINDOW`
   - huge monospaced countdown such as `07:42`
   - explicit target: `until ordinary Vermin are expected to return`
3. **Threat instruction**
   - `LOWER AMBIENT RISK`, `RETREAT WINDOW`, `HOSTILES ACTIVE`, or `RUPTURA INCOMING`
   - one action sentence: `Finish the POI and start disengaging`
4. **Opportunity shelf**
   - at most two high-value cards above the fold
   - each says what is favorable now, how long the window is expected to last, and whether it is confirmed, community-derived, player-observed, or experimental
5. **Next events and controls**
   - compact next-event list with exact local times
   - `Mute one cycle` and `Re-sync` as the only always-visible actions
   - deeper controls behind a sheet so an accidental tap cannot move the anchor

The hero countdown should remain readable across a room. Phase colors are supporting atmosphere, not the only signal:

- fire wave: deep red plus solid danger border and shelter icon;
- cooling: burnt orange plus descending-temperature mark;
- recovery: charcoal/indigo plus open-window mark;
- world active: dark green/teal plus explicit `HOSTILES ACTIVE` text;
- warning: amber becoming red plus a static retreat banner.

Use a mostly black OLED-friendly surface, restrained glow, large white numerals, and original phase symbols. Do not copy game audio or third-party icons into a public build. The current corpus/source assets remain private-use only until licensing is resolved.

### Timeline treatment

Keep one proportional whole-cycle rail for orientation, but do not use it as the only phase selector. Add equal/minimum-width labeled phase steps or a next-events list so Burning and Cooling never collapse to a few pixels.

### Docked and system surfaces

- Portrait: hero countdown, threat, and two opportunities.
- Landscape/charging: countdown on the left; threat and opportunity on the right; no setup controls.
- Lock Screen/Dynamic Island: current state plus countdown to the one currently armed event.
- StandBy: across-the-room countdown and one action line.
- Apple Watch later: next event, threat, and a haptic warning.

## Opportunity model

Opportunity guidance should be data, not hardcoded prose in the view:

```text
OpportunityWindow
  id
  start event or offset
  end event or offset
  action and caveat
  evidence class and confidence
  applicable game build
  applicable source snapshot
```

Initial cards:

| Opportunity | Display window | App wording and caveat |
|---|---|---|
| Reduced ordinary POI population | Recovery | `Lower ambient threat - triggered encounters may persist` |
| Oxallop | Early recovery while lakebeds are dry | `Dry lakebeds exposed - gather before water returns` |
| Cave access, Quartz, Glowcaps | Recovery | `Caves favorable now - check, not guaranteed respawn` |
| Ignitium | Immediately after cooling through recovery | `Fresh post-wave gatherable - spawn rate may vary` |
| Star Tears | Late recovery, experimental timing | `Leave some cooled Ignitium if Star Tears are the priority` |
| Coralion/Vulpir eggs | Post-wave recovery | `Post-wave egg opportunity - spawn is variable` |
| Surface foliage and water | Regrowth/active world | `Regrowth underway` rather than an exact species timer |
| Sulheart | Fully regenerated world | Hide until its exact current timing is validated |

The Ignitium/Star Tears tradeoff is particularly useful: strong community evidence says Star Tears mature on unharvested cooled Ignitium. The app should surface the choice rather than merely list both items as simultaneously available.

Personalization can remain lightweight in v1: pin two priorities. The current player model suggests Oxallop and Prickler are higher-value reminders than already-overstocked Hydrobulb and Prism Herb, while Glowcaps remain a strategic medicine reserve.

## Alert ladder

Enemy return and Ruptura use distinct sounds and haptic signatures. Never rely on color or audio alone.

Recommended default enemy-return ladder:

| Point | Visual | Foreground cue | Background behavior |
|---|---|---|---|
| 5 minutes | Amber edge, `Finish or leave hard POIs` | short double haptic, optional tone | normal local notification |
| 2 minutes | Persistent retreat banner | double tone and haptic | optional normal notification |
| 1 minute | Red instruction, seconds emphasized | stronger distinct cue | off by default to avoid duplication |
| 15 seconds | static danger border and large seconds | one final cue; no ticking | none |
| Event | `ORDINARY VERMIN RETURNING` | urgent triple cue | local notification or explicitly armed alarm |

Recommended pre-Ruptura ladder:

- optional 10-minute planning notice;
- 5-minute `finish the trip` warning;
- 2:30 warning-phase start;
- 1 minute;
- 30 seconds;
- 15 seconds;
- fire-wave impact.

Better than fixed thresholds alone: let the player choose an **exit buffer**. A hard POI with a seven-minute disengagement buffer should produce `TURN BACK NOW` seven minutes before the expected enemy-return event.

Controls:

- `Full`, `Haptic only`, and `Visual only` profiles;
- separate phase, enemy, Ruptura, and opportunity toggles;
- `Mute one cycle`;
- sound/haptic preview before the first Watch Session;
- never replay a backlog of missed cues after resume; collapse to the strongest current-state message.

## Synchronization and trust

### Manual v1

- Primary action: `Fire wave hit now`.
- Alternate validated anchors later: `Cooling began now`, `Recovery began now`, `World active now`.
- Fine adjustment: `-5`, `-1`, `+1`, and `+5 seconds`.
- Always-visible `Re-sync`.
- Single-player pause/resume control.
- Dedicated-server mode must account for official pause-when-empty behavior.
- Absolute timestamps, never a decrementing stored counter.

Confidence labels:

- `Unsynced`
- `Manual sync - predicted`
- `Confirmed at one later transition`
- `Drift suspected`
- `Cycle data changed - re-sync required`

Do not invent a numerical accuracy estimate until it has been measured. Downgrade confidence after a large correction, device-clock change, model-version change, uncertain pause, save/reload, or server stop.

### Squad sharing

Share a complete versioned session payload, not only a start timestamp:

- anchor and selected cue;
- running/paused state and paused elapsed time;
- cycle definition version;
- game build/source snapshot;
- world/session identifier;
- last confirmation and confidence state.

Use QR, nearby transfer, or a short link. No account is required for the local-first version.

### Future exact sync

[Nhimself's MIT-licensed RuptureTimer mod](https://github.com/Nhimself/starrupture_timermod) reads the game subsystem/replicated server state and exports JSON every second with:

- current phase and remaining seconds;
- seconds to next Ruptura;
- wave number and wave type;
- paused state;
- per-phase timers when available.

It works for local/listen sessions and dedicated-server clients. A small opt-in Windows/LAN bridge can advertise that state to RuptureOps over the local network. This is a credible later path to exact phase, pause, and server-time synchronization without camera recognition. Treat the mod and SDK licenses separately before reusing code or redistributing binaries.

## Native iOS behavior

### Foreground Watch Session

Disable the iOS idle timer only while the monitor scene is active, then restore it. Provide a dim OLED mode and recommend charging for long sessions. Do not use background audio as a keep-alive mechanism.

### Background and lock screen

- Schedule local notifications at absolute dates and reschedule on any anchor/model/policy change.
- Compute state from `(now - anchor) modulo cycle duration` on every resume.
- Deduplicate by cycle number plus event ID.
- Use a Live Activity for the currently armed next event, not an indefinitely self-advancing promise without a background authority.
- Live Activities are suited to this 54-minute session and appear on Lock Screen, Dynamic Island, StandBy, Apple Watch, and other system surfaces, but are not permanent.

### iOS 26 loud alarm

AlarmKit is an unusually good fit for an explicit `Arm loud enemy-return alarm` action:

- it supports fixed alarms and countdowns;
- it can break through Silent mode and Focus;
- it appears on Lock Screen, Dynamic Island, StandBy, and a paired Apple Watch;
- it requires per-app authorization and a clear usage description.

Use AlarmKit only when the player deliberately arms the next event or Watch Session. Do not silently create an endless alarm every 54 minutes. On earlier iOS versions, use normal local notifications; Time Sensitive can be a separate opt-in. Critical Alerts are not appropriate for a game companion.

## Accessibility and operational safety

- Text, symbol, border shape, and optional pattern always accompany phase color.
- Support Dynamic Type, VoiceOver, high contrast, Differentiate Without Color, and Reduce Motion.
- No full-screen flashing red transition and no default per-second ticking sound.
- VoiceOver summary should be one stable sentence, not a per-second announcement.
- Haptics complement audio and visuals; they never replace them.
- Keep `Re-sync`, `Mute one cycle`, and the current capability state visible without hidden gestures.
- Display `Next alert at 9:42 PM` so the player can trust the system.

## MVP

1. Versioned cycle, event, and opportunity-window model.
2. Manual anchor sync, pause/resume, and fine adjustment.
3. Absolute-time engine with deterministic boundary tests.
4. Portrait and landscape Watch Session layouts.
5. Keep-screen-awake and dim OLED modes.
6. Visual, audio, and haptic escalation for enemy return and Ruptura.
7. Scheduled local notifications and capability/permission status.
8. Two prioritized opportunity cards with visible provenance.
9. Live Activity for the currently armed event.
10. iOS 26 AlarmKit enhancement behind an explicit arm action.

Not MVP:

- account/cloud sync;
- automatic map routing;
- Base Core attack or infection prediction;
- unverified exact plant respawn formulas;
- full PC helper/mod integration;
- Apple Watch-native application;
- multiple saved worlds beyond the simplest session selector.

## Validation before claiming exactness

Record at least several Hotfix 0.2.8 cycles in solo, hosted co-op, and dedicated-server play:

- local fire-wave impact time;
- Cooling and Recovery transitions;
- first ordinary enemy repopulation near a known POI;
- cave-vine closure;
- Ignitium, Oxallop, Star Tear, and egg transitions;
- warning-phase cues;
- pause/menu behavior;
- save, quit, reload, host leave, and empty dedicated-server behavior;
- whether difficulty or world settings change any duration.

Until then, use `predicted`, `expected`, `favorable`, and `lower threat`, never `exact`, `safe`, or `guaranteed` for community-derived transitions.

## Sources

Game and timer:

- [starrupture.tools Rupture Time Tracker](https://starrupture.tools/timer)
- [Open-source RuptureTimer mod](https://github.com/Nhimself/starrupture_timermod)
- [Official Update 1 notes](https://store.steampowered.com/news/app/1631270/view/490464385050870875)
- [Official Hotfix 0.2.1](https://store.steampowered.com/news/app/1631270/view/541135584198923067)
- [Official Hotfix 0.2.2](https://store.steampowered.com/news/app/1631270/view/496100856889868382)
- [Official Hotfix 0.2.3](https://store.steampowered.com/news/app/1631270/view/694259874864824744)
- [Official Hotfix 0.2.5](https://store.steampowered.com/news/app/1631270/view/659358246189924378)
- [Community timing observations](https://www.reddit.com/r/StarRupture/comments/1qf4a19/rupture/)
- [Ten-minute quiet-window Steam discussion](https://steamcommunity.com/app/1631270/discussions/0/798963496102451972/)
- [Respawn-timing Steam discussion](https://steamcommunity.com/app/1631270/discussions/0/682992000385982038/)
- [Plant guide](https://blogs.plitch.com/en/blog/starrupture-plants-guide-location-uses)
- [Cave reference](https://starrupture.wiki.gg/wiki/Cave)
- [Star Tears on Ignitium observations](https://www.reddit.com/r/StarRuptureGame/comments/1q9m339/just_figured_out_why_i_can_never_find_any_star/)

Apple platform:

- [Keeping the display awake](https://developer.apple.com/documentation/uikit/uiapplication/isidletimerdisabled)
- [Scheduling local notifications](https://developer.apple.com/documentation/usernotifications/scheduling-a-notification-locally-from-your-app)
- [ActivityKit](https://developer.apple.com/documentation/ActivityKit)
- [Live Activities design guidance](https://developer.apple.com/design/human-interface-guidelines/live-activities)
- [AlarmKit sample](https://developer.apple.com/documentation/AlarmKit/scheduling-an-alarm-with-alarmkit)
- [WWDC25 AlarmKit session](https://developer.apple.com/videos/play/wwdc2025/230/)
- [Accessibility guidance](https://developer.apple.com/design/human-interface-guidelines/accessibility/)


Created: 2026-06-04
Updated: 2026-06-18 16:30 PDT
Status: Logbook profile subpages in active implementation; Seals target locked and live on the Nyx dev stack

![[Research/assets/logbook-profile-north-star-2026-06-04.png]]

## Purpose
Logbook is the Charlemagne profile and session-summary project: a Guardian profile surface with recent play sessions, representative achievements, commendations, and comments.

## Current focus
Keep Milestone 2 session-summary validation stable, then implement the locked Milestone 3 timeline session-detail target in the dedicated Logbook SPA/backend worktrees.

Recent Pantheon evidence and taxonomy notes: [[Projects/Charlemagne/Research/Logbook Pantheon 2.0 research 2026-06-09|Logbook Pantheon 2.0 research - 2026-06-09]].

Recent Pantheon production materialization note: [[Projects/Warmind/Pantheon materializer backlog investigation - 2026-06-18|Pantheon materializer backlog investigation]] separates queue-stack symptoms from row backlog and records the serial `pantheon_pgcr_materialize` work shape.

Recent raid/Pantheon design direction and mock sets: [[Projects/Charlemagne/Research/Logbook raid profile design direction - 2026-06-08|raid profile design direction]], [[Projects/Charlemagne/Research/assets/logbook-pantheon-shape-options-2026-06-10/README|Pantheon shape options]], [[Projects/Charlemagne/Research/assets/logbook-pantheon-honest-data-options-2026-06-10/README|Pantheon honest-data options]], [[Projects/Charlemagne/Research/assets/logbook-pantheon-activity-raidlike-options-2026-06-10/README|Pantheon activity/raidlike options]], [[Projects/Charlemagne/Research/assets/logbook-pantheon-activity-catalog-refinement-2026-06-10/README|selected Activity Catalog refinement]], [[Projects/Charlemagne/Research/Logbook Pantheon difficulty and Custom scoring research 2026-06-10|Pantheon difficulty and Custom scoring research]], and [[Projects/Charlemagne/Research/assets/logbook-pantheon-difficulty-refinement-2026-06-10/README|Pantheon difficulty refinement mocks]].

Recent seasonal/Monument design direction: [[Projects/Charlemagne/Research/Logbook seasonal Monument checklist MOCs 2026-06-10|Logbook seasonal Monument checklist MOCs - 2026-06-10]].

Recent SRL manifest evidence, 2026-06-09: Warmind dev manifest `244019.26.05.29.1640-4-bnet.65312` exposes an SRL triumph node under Monument of Triumph with 9 gameplay records and 21 objective/interval-objective tracked items when the aggregate `Competitions: SRL` checklist record is excluded. The raw exhaustive SRL collectible set found from SRL source strings plus the literal SRL emblem node is 43 collectibles: 15 armor class pieces, 6 weapons, 9 vehicles, and 13 cosmetics. A 32-item mockup total with 5 armor, 4 weapons, 8 vehicles, and 15 cosmetics is not raw-manifest exhaustive; use it only if the product intentionally collapses class armor slots and excludes/merges specific reward items.

SRL implementation note, 2026-06-10: SRL PGCR handling must prefer race-specific PGCR fields (`completed_race`, `race_completion_time`, `race_position`) over generic `completed`, `score`, or `standing`; generic completion/score can represent unfinished SRL races and must not award wins, podiums, completions, best times, or leaderboard credit. SRL route rendering should classify Bungie mode `94` before shared Crucible mode data, including PGCR detail payloads whose `modes` include both `5` and `94`. SRL profile and analytics surfaces should link best/recent/fastest races to `/pgcr/{instanceId}`. Clan SRL leaderboard comparison should reuse the existing guild leaderboard route/function path (`/in/rank/:guildDiscordId/:leaderboardId` and `/rank/:guildDiscordId/:leaderboardId`) with the SRL track leaderboard ids, not add a new SQL query.

## Priority recommendation - 2026-06-08
Current recommendation after branch audit and online research:
- Finish the core Logbook loop before starting SRL or Monument stats pages: Milestone 3 session detail, comments, inline PGCR expansion, deterministic share cards/unfurls, and browser QA.
- First profile subpage should be `/p/{profileId}/raids`, because the locked Milestone 4 route exists, the current profile has raid/dungeon summary data, and raid-specific sites prove the value of receipt-backed accomplishments.
- Treat Pantheon as a special raid-family module inside the raids work, not as a generic raid clear and not as the first standalone page. Warmind already has Pantheon rollup knowledge for the 2024 activity hashes, but Logbook still needs a proof-aware profile model for variants, limited-event context, and Pantheon 2.0 launch data.
- Keep SRL and Monument/Triumphs in design-shell or placeholder state until the post-2026-06-09 manifest, activity modes, record/presentation nodes, and first PGCR/API samples are verified.
- Use the existing activity-mode map/review path for new modes and rotators instead of hardcoding new profile semantics directly in summary cards.
- Resolve the self-commend product rule before launch. Current backend behavior allows signed-in users to commend their own published sessions; decide whether that is intended and align tests, API, and UI copy.

Recommended implementation order:
1. Finish and QA Milestone 3 session detail plus comments and inline PGCR expansion.
2. Ship deterministic Session Recap and PGCR Receipt share cards/unfurls as the acquisition loop.
3. Align this project doc with current code reality and lock public/private share behavior.
4. Build `/p/{profileId}/raids` with proof semantics: clears, full clears, first and latest clears, fastest full run, master/prestige, flawless/contest/low-man only when provable, source freshness, and PGCR receipts.
5. Add Pantheon as a special raid-family chapter with event-era context, per-tier completions, high-score/Platinum distinction where supported, and unknown/unavailable states where API evidence is missing.
6. After Monument launch data is live, run a focused mode/taxonomy discovery pass for Pantheon 2.0, SRL, Monument Triumphs, and collections/triumph progress before rendering authoritative progress.

## Implementation status
- 2026-06-05: Milestone 1 profile overview is implemented in Warmind/SPA on branch `codex/logbook-profile-planning` at `/p/{identifier}`.
- Backend route: `/in/logbook/profile/:identifier`.
- The page renders the public profile summary with no session information, matching the logged-in/logged-out Milestone 1 scope.
- Registered Charlemagne users return `ready`; unregistered/non-linked profiles return `registerRequired`; invalid identifiers return `notFound`.
- Current validated fixture: Tor_Kallon / Tor Kallon#6761, membership `4611686018428592074`.
- Final local validation captures:
  - Desktop: `/Users/aether/.codex/logbook-validation/logbook-m1-desktop-final-latest.png`
  - Mobile: `/Users/aether/.codex/logbook-validation/logbook-m1-mobile-final-latest.png`
- Current implementation validation captures after real Tor data wiring:
  - Desktop full page: `/Users/aether/.codex/logbook-validation/logbook-m1-desktop-current-full.png`
  - Mobile 390 full page: `/Users/aether/.codex/logbook-validation/logbook-m1-mobile-current-full.png`
  - Mobile 360 full page: `/Users/aether/.codex/logbook-validation/logbook-m1-mobile360-current-full.png`
  - Browser smoke found no error overlay and no horizontal overflow at 1440, 390, or 360px.
- Progress visual semantics are completion-focused only. Seasonal, campaign/exotic, seals, nightfalls, and collections use line progress; raids and dungeons use discrete dots. Stat-only cards do not emit progress bars.
- Campaign and exotic-mission counts are filled from Bungie profile record groups in Milestone 1. These are conservative, manifest-backed completion/main-quest records requested through the normal Bungie profile records path; the page does not scan raw PGCRs, `summary_story`, or large production summary tables.
- Campaign/exotic categories render `-- / --` when the profile is private, Bungie records are unavailable, or any required record group cannot be found. Missing data should not be displayed as known incomplete.
- Future work can replace the record-group source with a richer persisted summary if we want comprehensive campaign-era coverage, but avoid adding seeded state unless a live helper cannot answer the question.
- 2026-06-10: `/p/{identifier}/seals` implemented across Warmind API and SPA. It exposes completed seals sorted rarest first with rarity visible, plus a closest-to-finishing section only when Bungie record objective progress is available. The profile Seals summary card now links to this subpage.
- 2026-06-10 18:13 PDT: Seals locked target is the Profile/SRL desktop+mobile refinement at `Research/assets/logbook-seals-page-2026-06-10/logbook-seals-page-profile-srl-desktop-mobile.png`. Live Nyx dev route `http://127.0.0.1:13091/p/tor-kallon/seals` passed visual QA against real Tor Kallon data; comparison artifact: `/Users/Shared/projects/spa-logbook-profile/design-qa/logbook-seals/reference-vs-live-desktop-mobile.png`.

## Design artifacts
- North star image: ![[Research/assets/logbook-profile-north-star-2026-06-04.png]]
- Locked desktop HTML: ![[Research/assets/logbook-profile-html-desktop-locked-2026-06-04.png]]
- Locked mobile HTML: ![[Research/assets/logbook-profile-html-mobile-locked-2026-06-04.png]]
- Desktop source/code comparison: ![[Research/assets/logbook-profile-html-desktop-comparison-locked-2026-06-04.png]]
- Narrow mobile locked smoke: ![[Research/assets/logbook-profile-html-mobile-360-smoke-2026-06-04.png]]
- Local HTML mock: `/Users/aether/.codex/generated_images/019e8f8f-2dc8-7af1-936f-7d64b2504f0f/logbook_profile_mock.html`
- Design principles and milestone mocks: [[Projects/Charlemagne/Research/Logbook profile design principles and milestone mocks - 2026-06-04]]
- Milestone mock source: `/Users/aether/.codex/generated_images/019e8f8f-2dc8-7af1-936f-7d64b2504f0f/logbook_milestone_mocks.html`
- Milestone desktop contact sheet: ![[Research/assets/logbook-milestone-desktop-contact-sheet-2026-06-04.png]]
- Milestone mobile contact sheet: ![[Research/assets/logbook-milestone-mobile-contact-sheet-2026-06-04.png]]
- Milestone 3 current lead with PGCR share: ![[Research/assets/logbook-m3-session-detail-current-lead-with-pgcr-share-2026-06-05.png]]
- Milestone 3 balanced command-center option: ![[Research/assets/logbook-m3-session-detail-balanced-command-center-2026-06-05.png]]
- Milestone 3 activity-inspector option: ![[Research/assets/logbook-m3-session-detail-activity-inspector-2026-06-05.png]]
- Milestone 3 locked timeline detail target: ![[Research/assets/logbook-m3-session-detail-timeline-locked-2026-06-07.png]]
- Raid grounded mocks: [[Projects/Charlemagne/Research/Logbook raid grounded mocks 2026-06-08]]
- Raid current evolution variants: [[Projects/Charlemagne/Research/Logbook raid current evolution variants 2026-06-08]]
- Seals page concept: ![[Research/assets/logbook-seals-page-2026-06-10/logbook-seals-page-concept.png]]
- Seals Profile/SRL desktop+mobile refinement: ![[Research/assets/logbook-seals-page-2026-06-10/logbook-seals-page-profile-srl-desktop-mobile.png]]
- Seals live comparison: `/Users/Shared/projects/spa-logbook-profile/design-qa/logbook-seals/reference-vs-live-desktop-mobile.png`

## Notes
- Sessions should be representative, not only highlight reels.
- Most sessions should show minor achievements or none; streaks can appear as achievements.
- Commendations are the lightweight positive reaction term for session likes.
- Session comment buttons should expand an inline comment field without navigation or reload.
- Desktop and mobile HTML captures are locked as the profile north stars as of 2026-06-04.
- Mobile locked captures were regenerated with true CDP device metrics; 390px and 360px both measured `scrollWidth == clientWidth` with zero horizontal overflow offenders.
- Mobile includes a top-bar-only `Sessions` jump link so users can skip the long summary-card stack and land directly on recent sessions once sessions are part of the page.
- Session highlight cards are removed for now; the achievement count lives in the session metrics and achievement chips are supporting detail.
- Seasonal and SRL summary cards are part of the current target design.
- As of the 2026-06-05 M1 refinement, summary cards are intentionally grouped as general things -> PvE things -> PvEvP thing -> PvP things. Preserve this ordering unless product direction changes.
- Current card order: Seasonal, Campaigns & Exotic Missions, Collections, Seals, Raids, Dungeons, Nightfalls, Gambit, Crucible, Trials of Osiris, Iron Banner, SRL.
- Collections and Seals live in the second desktop row so broad completionist identity appears before mode-specific PvE stats. The four Crucible-family cards stay adjacent at the end: Crucible, Trials, Iron Banner, SRL. Cards without an honest completion goal render without a progress bar.
- Revisit PvP/Gambit completion goals after the 2026-06-09 Destiny update/Monument surface is visible. Candidate sources should be curated Bungie triumph/record groups, not raw Triumph Score buckets or active-time share.
- Production profile/session imagery should come from Charlemagne/Bungie manifest assets already stored in Warmind Redis/disk, with generated or edited imagery used only where source assets are missing.
- The requested longer ImageGen pass was blocked by the ImageGen service on 2026-06-04; retry before treating the north-star image as final.

## Visual semantics
- Class-mix bars show lifetime active-time share by class; the strongest main gets the dominant bar.
- Class-mix labels should only call out the main class. Do not label the others as `alt`.
- Session activity-mix bars show active-time share by activity family inside that session. Label only the meaningful segments; leave small remainder segments quiet.
- Summary-card dots show discrete checklist-style progress. Lit dots are completed/earned/cleared buckets; dim dots are remaining buckets. Current dots are raid and dungeon completion coverage.
- Summary-card solid lines show continuous progress for the primary card concept. Current lines are seasonal rank progress to 100, campaign/exotic completion coverage, available-seal coverage, GM Nightfall coverage, and tracked-collection coverage.
- Summary-card progress indicators must have hover tooltips explaining the exact meaning. Do not show an indicator on placeholder or stat-only cards unless the backend has an honest completion denominator or explicit goal.
- Seals display earned counts without a static all-time denominator. Seal progress must count only currently available/completable seals; the first implementation filters the manifest seal records to visible, titled, non-redacted, non-blacklisted, non-expiring records.
- Seasonal uses season rank progress toward 100 when that stat is available.
- Nightfalls use the manifest-computed Grandmaster Nightfall activity list and the player's existing `summary_nightfalls` successes. If the manifest-computed GM list or per-activity Nightfall detail rows are unavailable, the rail should be absent rather than guessed.
- Collections use Bungie profile collectible state to count visible tracked collectibles. The exact count belongs in the tooltip; the visible metric should stay compact.
- SRL now has a post-Monument registered-user completion source from the 2026-06-09 manifest pass: use 21 SRL triumph tracked items and 43 exhaustive SRL collectibles for Charlemagne users; render `--` for non-registered users instead of trying to derive these from public profile fallback dataer without a progress bar.
- Revisit PvP/Gambit completion goals after the 2026-06-09 Destiny update/Monument surface is visible. Candidate sources should be curated Bungie triumph/record groups, not raw Triumph Score buckets or active-time share.
- Production profile/session imagery should come from Charlemagne/Bungie manifest assets already stored in Warmind Redis/disk, with generated or edited imagery used only where source assets are missing.
- The requested longer ImageGen pass was blocked by the ImageGen service on 2026-06-04; retry before treating the north-star image as final.

## Visual semantics
- Class-mix bars show lifetime active-time share by class; the strongest main gets the dominant bar.
- Class-mix labels should only call out the main class. Do not label the others as `alt`.
- Session activity-mix bars show active-time share by activity family inside that session. Label only the meaningful segments; leave small remainder segments quiet.
- Summary-card dots show discrete checklist-style progress. Lit dots are completed/earned/cleared buckets; dim dots are remaining buckets. Current dots are raid and dungeon completion coverage.
- Summary-card solid lines show continuous progress for the primary card concept. Current lines are seasonal rank progress to 100, campaign/exotic completion coverage, available-seal coverage, GM Nightfall coverage, and tracked-collection coverage.
- Summary-card progress indicators must have hover tooltips explaining the exact meaning. Do not show an indicator on placeholder or stat-only cards unless the backend has an honest completion denominator or explicit goal.
- Seals display earned counts without a static all-time denominator. Seal progress must count only currently available/completable seals; the first implementation filters the manifest seal records to visible, titled, non-redacted, non-blacklisted, non-expiring records.
- Seasonal uses season rank progress toward 100 when that stat is available.
- Nightfalls use the manifest-computed Grandmaster Nightfall activity list and the player's existing `summary_nightfalls` successes. If the manifest-computed GM list or per-activity Nightfall detail rows are unavailable, the rail should be absent rather than guessed.
- Collections use Bungie profile collectible state to count visible tracked collectibles. The exact count belongs in the tooltip; the visible metric should stay compact.
- SRL remains a no-bar placeholder until the post-Renegades/SRL manifest exposes a reliable completion goal.
- Thin vertical dividers separate metric groups only; they do not encode score or progress.
- Achievement chips are earned session-level moments. The numeric achievement metric is the total; chips are representative examples, and a session can have none.

## Prod fixture data needed
The current Tor dev fixture is missing some profile-card summary rows, so M1 intentionally shows `--` for unavailable values rather than inventing data. Prefer fetching a narrow production fixture over adding synthetic rows.
- The 2026-06-05 Nyx Tor fixture check confirmed `charlemagne.stats` has time rows such as active/PvP/Iron Banner/Gambit/Trials, but no `stat_type_id = 497` season-rank row. The Seasonal 100-rank rail will stay absent until that stat is present.

Prompt to run from Erebus/prod-capable context:

```text
In Warmind, create a narrow dev fixture export for Logbook profile validation for Tor_Kallon, membershipId 4611686018428592074 and Charlemagne profile id/user rows. Keep it read-only against prod and export only the rows needed to hydrate the M1 profile cards in Nyx dev: charlemagne user/profile rows, charlemagne.stats rows for this profile including stat_type_id 497 season rank, nexus.bungie_profiles, nexus.clans for clan 223562, nexus.seals and nexus.seals_gilded for this membership, nexus.character_profiles/class-time source rows, nexus.summary_atime rows needed for current seasonal active time, nexus.summary_crucible rows for all PvP, Trials, and Iron Banner all-time/current-season K/D and win rate, nexus.summary_gambit and nexus.summary_gambit_prime rows for win rate/motes, and nexus.summary_nightfalls rows if needed for Nightfall time. Do not dump broad tables. Produce an importable SQL file or documented mysql commands for Nyx dev.
```

## Locked URL structure
Route decision, locked on 2026-06-04:
- Public profile: `/p/{profileId}`.
- Premium vanity profile: `/p/{slug}` using the same route with a resolver that accepts either a stable profile id or a reserved vanity slug.
- Session detail: `/p/{profileId}/s/{sessionId}`.
- Profile subpages: `/p/{profileId}/raids`, `/p/{profileId}/crucible`, `/p/{profileId}/collections`, etc.

Rationale:
- Existing SPA routes already use `/s` for server/guild pages and `/u` for user pages, so top-level `/s/{sessionId}` and `/u/{profileId}` are off the table.
- `/p` is currently free in the SPA route tree, reads as profile, and is less cryptic than `/g`.
- `profileId` should be a stable Charlemagne public profile id, not a mutable Bungie display name.
- Sharing one `/p/{identifier}` route for ids and premium slugs keeps vanity URLs short without opening a top-level wildcard route.
- Nesting session detail under `/p/{profileId}/s/{sessionId}` avoids the existing `/s` server/guild route while keeping the relationship to the Guardian clear.

Rejected or deferred route shapes:
- `/g/{profileId}` is currently free, but `/p` is clearer for a web profile and avoids sitting near `/gambit` and `/global`.
- `/@/:slug` is route-safe in React Router but longer and less natural than the desired vanity shape.
- `/@slug` is attractive, but React Router did not match `/@:slug` in a quick local check; it would likely need edge rewrite or custom parsing.
- `/{slug}` is the shortest premium vanity URL, but it risks collisions with many existing top-level routes and future product pages.
- `/p/{profileId}/sessions/{sessionId}` is clearer than `/p/{profileId}/s/{sessionId}` but longer.

Route audit notes from the existing SPA:
- Taken top-level prefixes include `/s`, `/u`, `/reg`, `/register`, `/login`, `/logout`, `/subscribe`, `/donate`, `/article`, `/articles`, `/cmd`, `/cmds`, `/faq`, `/privacy`, `/tos`, `/about`, `/screenshots`, `/history`, `/posts`, `/info`, `/maps`, `/lost-sectors`, `/community`, `/leaderboards`, `/leaderboard`, `/weapons`, `/emblems`, `/triumphs`, `/gm`, `/grandmaster`, `/pve`, `/destiny2`, `/activity`, `/pvp`, `/crucible`, `/gambit`, `/nf`, `/nightfall`, `/trials`, `/fireteam`, `/raid`, `/raids`, `/dungeon`, `/dungeons`, `/rank`, `/drank`, `/global`, and `/analytics`.
- `/p` and `/g` are not currently SPA route prefixes.
- `/@me` exists under the SPA API path (`/spa/@me`), not as a browser route.

## Implementation guardrails
- Before writing a new helper, search for an existing function, route, DAO, cache helper, manifest helper, or component that already owns the behavior.
- Follow existing Warmind and SPA conventions. Fit into the current package/component boundaries unless there is a concrete reason to add something new.
- Follow repo `AGENTS.md` and the Charlemagne coding standards note for Go style, API boundaries, and implementation shape.
- Be surgical. Do not refactor existing systems, rename broad APIs, or reorganize unrelated code without explicit permission.
- Prefer existing profile, Charlemagne-user, manifest, summary, and sessiontracking structures over new parallel concepts.
- Use compact profile/session read models for UI paths. Do not make page render paths scan raw PGCR tables or huge historical summary tables.
- Keep parser impact at the already-agreed minimal boundary: session enqueue after successful PGCR transaction, with the rest owned by cortex/sessiontracking.

## Demo and validation cases
Required real-account fixture:
- Tor_Kallon / Tor Kallon#6761, membership `4611686018428592074`, membership type `2`, clan `223562`. Use this as the primary recognizable profile fixture even when synthetic session data is needed. Current Nyx dev session table check on 2026-06-04 22:48 UTC showed `0` tracked sessions for this membership.

Recent live-edge session candidates from the current Nyx dev real-time edge session tables, checked on 2026-06-04 22:48 UTC:
- Ciris Silver#9867, membership `4611686018458772527`: 3 sessions, 7 activities, latest session started 2026-06-04 22:42:41 UTC.
- RubyTheMerman#2336, membership `4611686018455865802`: 3 sessions, 6 activities, 2h 43m active time in the 7-day sample, latest session started 2026-06-04 22:17:04 UTC.
- FactsNotFeelings#4104, membership `4611686018455150976`: 2 sessions, 8 activities, 1h 02m active time in the 7-day sample, latest session started 2026-06-04 22:23:19 UTC.
- Redacted7279#5884, membership `4611686018453128848`: known SPA fixture plus recent session rows; 3 sessions, 7 activities, latest session started 2026-06-04 22:08:46 UTC.
- ROSSCILLATOR#5122, membership `4611686018459379646`: 3 sessions, 16 activities, unusually long accumulated active time in the 7-day sample. Keep as a catch-up/session-boundary stress case.

Existing fixture accounts to reuse where they already fit:
- Disrok / Javi#1864, membership `4611686018443723150`, for Trials and Crucible-heavy direct-profile validation.
- Mustang_2009KR#1767, membership `4611686018444431647`, for current-week Crucible and synthetic fireteam validation.

Synthetic data needed to prove the product vision:
- A registered Charlemagne profile with no tracked sessions.
- A non-Charlemagne profile path that renders the registration prompt.
- Tor_Kallon sessions covering a normal sparse night, a mixed activity night, and an achievement-heavy night.
- A logged-out profile view that matches the logged-in view for Milestone 1.
- Milestone 2 session data for logged-out latest-only behavior, logged-in multi-session behavior, and disabled logged-out commend action.
- Sessions with zero achievements, a few minor achievements, many achievements, and streak achievements.
- Milestone 3 detail fixtures with multiple PGCRs, comments, and session rename permissions.

## Milestones
### Milestone 1: Profile Overview
- Build only the profile summary surface; no session information.
- Do not include links to profile subpages yet.
- Logged-in and logged-out profile views should render the same public profile when the Guardian is already a Charlemagne user.
- If the Guardian is not a Charlemagne user, show a registration prompt explaining that they need to register so the profile can populate.

### Milestone 2: Session Summaries
- Add recent session summaries to the profile page.
- Include commend counts and the ability to commend.
- Do not include comments yet.
- Logged-out users see only the latest session plus a prompt to log in to see more.
- Logged-out users cannot commend.

#### Detailed implementation plan

Milestone 2 adds closed session summaries to the bottom of the existing `/p/{identifier}` profile page. Keep this as a profile-page enhancement, not a new navigation surface. Comments, session detail pages, rename controls, PGCR drilldown, and profile subpages stay out of this milestone.

Backend shape:
- Do not fold sessions into the cached `/in/logbook/profile/:identifier` profile response. Profile data is cached and mostly viewer-independent; sessions and commends are mutable and viewer-sensitive.
- Add a public latest-session endpoint under the existing internal profile route family, likely `GET /in/logbook/profile/:identifier/sessions/latest`. It resolves the same profile identifier as the profile endpoint and returns at most one public-visible session. Logged-out users should not be able to request a larger page by changing a `limit` parameter.
- Add an authenticated read endpoint under the existing SPA route family, likely `GET /spa/logbook/profile/:identifier/sessions?limit=5`. This uses the existing JWT middleware and returns more recent sessions for the requested profile, plus viewer-specific `viewerCommended` flags.
- Keep public and authenticated reads separate because the current SPA JWT middleware is hard-auth only; adding optional-auth plumbing to `/in` is not necessary for Milestone 2.
- Public visibility must be explicit. Finalization currently writes ordinary closed sessions to `ready`; that is not enough to make them publicly visible. For Milestone 2, logged-out and non-owner reads should only expose sessions in an explicit public state such as `published`, or behind a named Logbook sharing flag if we add one. The owner can see their own ready sessions while signed in.
- Add an authenticated commend write endpoint, likely `POST /spa/logbook/sessions/:sessionId/commend`. It is idempotent and returns the updated count and viewer state. It must reject sessions the viewer cannot see, not just missing sessions. For Milestone 2, self-commends should be disabled unless product direction changes; the UI can show the count but not an active self-commend action.
- Keep the existing private `/spa/sessions` and `/spa/sessions/:sessionId` endpoints in place for owner-owned session work. Do not repurpose them for the public profile feed.
- Do not use the proxy endpoints. Do not query raw PGCR JSON, weekly summary tables, or historical activity tables on profile render.

Session read model:
- Add a Logbook-specific DTO instead of returning `sessiontracking.SessionAPIResponse` directly. The public profile card should expose only the fields the page needs:
  - `id`, `startedAt`, `endedAt`, `activeSeconds`, `activityCount`, `primaryModeFamily`;
  - generated display title for now, not a persisted editable name;
  - `activityMix[]` with mode family, label, color, seconds/count fallback, and percent;
  - `achievementCount` plus a short `achievements[]` chip list;
  - `commendCount` and `viewerCommended`;
  - optional `playedWith` summary only when it can be produced safely from explicit session-summary fields;
  - no comments, comment counts, activity-event list, PGCR list, rename state, or moderation fields.
- Prefer reading from `charlemagne.play_sessions.summaryJson` and `achievementsJson`. Add compatibility handling for older summaries that lack new fields.
- Extend `sessiontracking.SessionSummary` during finalization to include activity-family active seconds, not just family counts. The UI needs time share for the session activity-mix bar. Existing summaries can fall back to counts until naturally recalculated or explicitly re-finalized.
- Defer participant avatar/name chips and clanmate counts unless the privacy and visibility rules are explicit. Milestone 2 can ship with no participant row or a conservative count-only row. If we include a count, it must come from compact finalization output or a bounded same-session lookup, not from a new broad participant system.
- Use deterministic titles only: examples like `Friday Night Fireteam`, `Raid Night`, `Crucible Session`, or `Reset Night` can be generated from start time, primary mode family, and mix. Persisted custom names wait for Milestone 3.

Commends data:
- Add `charlemagne.session_commends`.
- Proposed columns: `sessionId BIGINT UNSIGNED NOT NULL`, `userId BIGINT UNSIGNED NOT NULL`, `createdAt DATETIME NOT NULL`.
- Proposed indexes: primary key `(sessionId, userId)`, `KEY user_created (userId, createdAt)`, and `KEY session_created (sessionId, createdAt)`.
- Count commends with a small indexed query over the session IDs returned for the page: `sessionId IN (...) GROUP BY sessionId`. Compute `viewerCommended` with `(sessionId, userId)` lookups using the primary key; do not implement it as a user-first scan.
- Do not add denormalized counts until read volume proves it is needed.
- Update the current session cleanup seams explicitly: `DeleteSessionTrackingForUserTX`, `DeleteSessionTrackingForBNetIDTX`, and `DeleteSessionTrackingForMembershipID`. Delete `session_commends` rows before deleting `play_sessions` so user deletion, BNet unlink, and profile unlink all remove commends made by the user and commends attached to deleted sessions.
- The insert path should use `INSERT IGNORE` or equivalent idempotency so double-clicks and retries cannot double-count.

SPA shape:
- Extend `src/api/warmind/Logbook.ts` with typed session DTOs and functions for:
  - latest public session;
  - authenticated profile sessions;
  - authenticated commend.
- Use uncached API calls for session reads and commend writes. The existing cached profile helper is fine for the profile shell, but mutable session/commend data should not use `GetCachedWarmindApiResponse`.
- After a successful commend, update local session-card state directly; do not depend on the profile cache refreshing.
- Keep `/p/:identifier` as the only browser route for Milestone 2.
- In `LogbookProfile.tsx`, keep the existing hero and summary-card structure stable. Add a `Sessions` section after all summary cards and give it an anchor target for the mobile jump.
- Use the locked Milestone 2 mocks as visual reference:
  - desktop: `Research/assets/logbook-milestone-2-sessions-desktop-2026-06-04.png`;
  - mobile: `Research/assets/logbook-milestone-2-sessions-mobile-2026-06-04.png`;
  - logged-out: `Research/assets/logbook-milestone-2-logged-out-desktop-2026-06-04.png` and `Research/assets/logbook-milestone-2-logged-out-mobile-2026-06-04.png`.
- Session cards should be spacious enough to avoid the crowding fixed during the north-star pass. The desktop action area should be quiet and right-aligned; mobile should stack actions below the session facts without a large dominant button.
- Use a small thumbs-up/commend icon inside the commend button. Keep the button less loud than the session title and metrics.
- Omit comment buttons, comment counts, and inline comment fields entirely until Milestone 3.
- Logged-out UI shows the latest session and a concise login prompt such as "Sign in to see more sessions and commend." The disabled/comment-free state should not look broken.
- Logged-in UI shows multiple recent sessions and allows commending sessions the viewer is allowed to commend. After a successful commend, update the local session row state immediately.
- Empty state: show a quiet sessions section saying the Guardian has no closed tracked sessions yet, not an error.

Query and production-safety guardrails:
- Public/session list reads should use `play_sessions.membership_started` and only return explicit public-visible sessions.
- Authenticated multi-session reads should use `play_sessions.membership_started`, return owner-visible sessions for the signed-in owner, return only public-visible sessions for non-owners, and clamp limits with the existing route helper if one fits. A small default such as 5 and a hard cap such as 10 are enough for the profile page.
- Commend count reads should be limited to the returned session IDs and should use the `session_created` index.
- No request path should scan `session_activity_events` except as a bounded fallback for a tiny returned session set, and only if summary JSON is missing a field.
- No request path should call Bungie, query raw PGCR blobs, or scan the large weekly/daily historical tables.
- Keep parser impact at zero for this milestone. Parser already enqueues; all new work belongs in sessiontracking, warmindapi, charlemagne migration/cleanup, and SPA.

Tests and validation:
- Backend tests:
  - profile-session latest endpoint returns only one public-visible session for logged-out/public reads;
  - logged-out and non-owner reads cannot see ordinary owner-only `ready` sessions;
  - authenticated profile-session endpoint returns multiple owner-visible sessions and viewer commend state for the owning user;
  - non-Charlemagne/unregistered profile behavior matches profile endpoint behavior;
  - commend POST requires auth, is idempotent, updates count, and rejects missing, non-visible, non-ready/non-public, and self sessions according to the chosen rule;
  - cleanup helpers remove commends made by a deleted user and commends on deleted sessions;
  - summary decoding supports both old and new summary JSON.
- Sessiontracking tests:
  - new summary fields aggregate activity-family active seconds correctly;
  - achievement count and chip-list selection tolerate zero, few, and many achievements.
- SPA tests:
  - logged-out profile shows only latest session plus login prompt and no active commend action;
  - logged-in profile shows multiple sessions and can optimistically update a commend count;
  - session cards render zero-achievement and multi-achievement sessions without layout breakage;
  - no comments UI appears in Milestone 2.
- Browser validation:
  - desktop and mobile screenshots should match the locked Milestone 2 mocks with data differences only;
  - check 1440px, 390px, and 360px widths for `scrollWidth == clientWidth`;
  - verify the mobile `Sessions` jump lands at the session section and does not obscure the first card.
- Data validation:
  - Use Tor_Kallon as the recognizable profile fixture, with synthetic session rows if production session history is still sparse.
  - Use at least one live-edge active session candidate from the existing list for real parser/finalizer shape.
  - Include synthetic cases for no sessions, ordinary session with zero achievements, mixed session, achievement-heavy session, and already-commended-by-viewer state.

Implementation review checklist:
- Search for existing helpers before adding new ones, especially identifier resolution, avatar URL formatting, profile/user lookup, cleanup, API response wrappers, and SPA auth utilities.
- Specifically reuse or extract the current Logbook profile identifier resolver before adding another resolver, and use the existing bounded integer query helper for limits if it fits the handler shape.
- Keep routes and DTOs named `logbook`-specific so they do not collide with the existing private session endpoints.
- Keep the profile page layout changes local to the Logbook component family.
- Run adversarial review against three risks before implementation is considered done: public data leakage, production-hot queries, and visual/mobile regression.

### Milestone 3: Session Detail And Comments
- Add session detail pages.
- Let users rename sessions.
- Show deeper session detail, including the specific PGCRs included in the session.
- Add comments.

#### Design checkpoint - 2026-06-05

Current direction:
- The strongest current direction is the session-detail command-center layout: existing Warmind/Charlemagne chrome, session header, summary blocks, PvP summary, spacious `Activities / PGCRs`, inline PGCR expansion, fireteam, achievements, and session-level comments.
- Keep the page grounded in the Milestone 2 profile/session visual language. Do not introduce a new sidebar, new top nav, marketing hero, or unrelated social-feed surface.
- Option 3/fireteam-first is rejected for now. The session is primarily about the individual Guardian; many sessions include random teammates, so the fireteam should support the story rather than own the page.
- The current lead is `Research/assets/logbook-m3-session-detail-current-lead-with-pgcr-share-2026-06-05.png`.
- The two useful alternate directions are:
  - `Research/assets/logbook-m3-session-detail-balanced-command-center-2026-06-05.png`;
  - `Research/assets/logbook-m3-session-detail-activity-inspector-2026-06-05.png`.

Product rules locked during the design pass:
- Comments attach to the session, not to individual PGCRs.
- Commends are a single session-level action/count. Do not show `top commends`; a viewer can only commend a session once, but the UI does not need explanatory copy for that.
- If a session includes PvP, always show an overall PvP stat summary for the session: K/D, efficiency, win rate, kills, deaths, and assists.
- If a session mixes Iron Banner, Trials, and general Crucible, also show stats by PvP mode.
- Each PGCR/activity row represents exactly one PGCR. Do not collapse two Crucible matches into one row that says `2 wins`.
- Clicking a specific PGCR expands it inline without a reload into a standard PGCR detail view: result, team scores, player rows, kills, deaths, assists, K/D, efficiency, and mode-specific objective stats such as captures.
- Later creative PGCR concepts such as raid MVP are interesting, but the first version should at least reach parity with ordinary Bungie/third-party PGCR detail expectations.
- Comments should have more breathing room than the dense table-like mock. Comments will be rarer than stats and should feel like a readable session conversation when they appear.

#### Locked timeline detail target - 2026-06-07

Implementation target:
- Use the locked timeline detail mock at `Research/assets/logbook-m3-session-detail-timeline-locked-2026-06-07.png`.
- Work in the dedicated Logbook worktrees and branch: SPA `/Users/Shared/projects/spa-logbook-profile` and backend `/Users/Shared/projects/warmind-logbook-profile`, both on `codex/logbook-profile-planning`.
- Keep the existing Warmind/Charlemagne SPA chrome, centered page lane, dark Warmind greys, orange accents, and current Logbook visual language.
- Keep the page information ownership clear: header owns title/subtitle and neutral facts, timeline owns PGCR sequence, expanded PGCR owns match detail, right rail owns context/actions, and comments own the conversation.
- Top metrics are only `Active Time` and `Activities`. Do not add a top-row People metric; people belong in the right-rail `Played With` context.
- Header chips are only activity families such as Raid, Vanguard, and Crucible. Achievements appear only in the right rail.
- Achievements should be a right-rail module that can grow downward and show as many as needed, with a `View all` affordance when truncated.
- Commends appear only as the session-level action/count, for example `Commend · 24`; do not add a separate Commends metric card.
- Comments appear only in the comments section heading, for example `Comments · 6`; do not add a separate Comments metric card.
- Do not add a right-rail PvP summary. PvP stats belong in the expanded PGCR detail and player tables.
- Preserve the calmer spacing from the final iteration: slightly taller timeline rows, more comment padding, and no card-in-card clutter.

#### Social Sharing And Social Cards - Open Discussion 2026-06-05

Status:
- This section records current product agreement and open iteration points. The visual direction, final card data, and implementation renderer are still being discussed.
- The key initial share objects are Session, PGCR, and Profile. Document the other card families as useful future ideas, but do not let them distract from the first three.
- Repo archive for the 2026-06-05 mock set: `/Users/Shared/projects/warmind-logbook-profile/docs/agents/logbook-social-sharing-card-mocks-2026-06-05.md`.
- Repo asset directory: `/Users/Shared/projects/warmind-logbook-profile/docs/agents/assets/logbook-social-sharing-2026-06-05/`.

Priority share objects:
- P0: Session Recap card. This is the session-as-memory share surface. It can include first clears, commends, meaningful moments, and social context, but it should not be locked to "night" language and the fireteam timeline concept is not yet accepted.
- P1: PGCR Receipt card. This is likely the strongest acquisition loop for people who are not already Charlemagne users: a player accomplishes something, shares the verified receipt, and brings others back to the site.
- P2: Guardian Legacy profile card. This is the profile-sharing card and should answer "what have you done, and what is your history?" It still has strong value for selling Logbook, but current iteration suggests the share loop should lead with sessions and PGCRs first. The visual style is close; the content needs one more endgame-focused revision, then profile-card work should pause while Session and PGCR cards get explored.

Share targets and staging:
- Stage 1 priority targets: Discord, X, and Bluesky. Discord should sit beside X and Bluesky for Destiny virality even though it does not have a clean web share intent; the answer for Discord is copy link plus a beautiful unfurl.
- Stage 1 should also keep Facebook in the metadata/support matrix because it still matters and was an explicit target.
- Stage 2 targets: Reddit, Steam Chat, iMessage/WhatsApp group chats, Threads, and Mastodon.
- Deferred for now: video-scrolling surfaces and short-video workflows. Do not plan TikTok, YouTube Shorts, Instagram Reels, or short recap videos until the core card/unfurl loop works.

Recommended technical shape:
- Every shareable object gets a stable public canonical URL with server-rendered metadata.
- Metadata should include Open Graph fields plus X/Twitter card fields: title, description, canonical URL, image URL, image width/height/type/alt, and `summary_large_image`.
- Dynamic share-card images should be deterministic and cacheable by object id plus render version. Per-instance card assembly must not be AI-powered; the system needs to scale to the large userbase.
- AI-generated or AI-assisted work is acceptable for component concepting, ornamental source assets, and design exploration, but production per-profile/per-session/per-PGCR card generation should use deterministic renderers and data-driven components.
- Keep the implementation renderer open for now. Candidate directions include HTML/CSS-to-image rendering, Canvas, Satori/resvg-style rendering, or another deterministic server-side image pipeline.
- Card images should be public, crawler-safe, and free of viewer-specific/private data because social crawlers cache first-seen results.
- Prefer JPG/PNG social-card output at the standard large-preview aspect ratio, with a compact fallback for platforms that crop or compress differently.
- Share UI should use native share where available, platform intent links where useful, and copy-link as the reliable baseline.

Card families:
- Guardian Legacy: the profile card. It should sell personal history, completion, identity, and nostalgia without becoming a dense stats poster. Current first cut should be endgame-focused.
- PGCR Receipt: the proof card. It should make accomplishments feel verified, portable, and worth sharing.
- Session Recap: the memory card. It should summarize the session as a human moment, not just a list of PGCRs.
- Monument Progress: good idea, especially while Monument of Triumph is current, but it follows the core profile/PGCR/session loop.
- Personal Best: useful later as a PGCR variant when the system can reliably identify season/career/activity bests.
- Fireteam Legacy: potentially powerful for tagging and nostalgia, but needs opt-in/privacy work before it becomes a major surface.

Open Guardian Legacy questions:
- Which facts best express "my Destiny history" without crowding the card?
- Which facts are reliable enough across registered and non-registered profiles?
- Should the card emphasize time played, first-seen/era span, seals/titles, raid/dungeon breadth, MoT progress, PvP/Gambit identity, clan/social history, rare achievements, or a small curated mix?
- What should be omitted because it feels spammy, generic, negative, too private, or too hard to verify?
- What is the quietest visual language that still feels exciting enough to share?

Guardian Legacy feedback after first mock pass:
- Style directions `01 Legacy Receipt` through `05 Career Arc` are directionally right.
- `06 Three Pillars` has useful colors and mood, but it drifts too far toward ordinary dashboard/stat-card treatment.
- Do not use envelope/letter metaphors for production cards.
- Use the real Warmind/Charlemagne logo and the real URL. The logo may be blended, textured, or treated less flat to fit the card style, but should not be redesigned.
- Use `warmind.io` as the public domain. Player profile URLs are `warmind.io/p/{guid}`; future vanity URLs should live in the same namespace.
- The first profile-card content pass needs more unique information than generic profile basics. Current candidate facts for an endgame-focused cut: Guardian Rank, years active, seals, raids, dungeons, and legendary campaigns.
- After one more Guardian Legacy revision, pause profile-card work and shift design focus to Session Recap and PGCR Receipt while keeping the same overall style system.

Current Guardian Legacy mock pass:
- Contact sheet: ![[Research/assets/logbook-social-guardian-legacy-contact-sheet-2026-06-05.jpg]]
- Directions generated for iteration:
  - `01 Legacy Receipt`: ![[Research/assets/logbook-social-guardian-legacy-01-legacy-receipt-2026-06-05.png]]
  - `02 Era Passport`: ![[Research/assets/logbook-social-guardian-legacy-02-era-passport-2026-06-05.png]]
  - `03 Title Shelf`: ![[Research/assets/logbook-social-guardian-legacy-03-title-shelf-2026-06-05.png]]
  - `04 Monument Line`: ![[Research/assets/logbook-social-guardian-legacy-04-monument-line-2026-06-05.png]]
  - `05 Career Arc`: ![[Research/assets/logbook-social-guardian-legacy-05-career-arc-2026-06-05.png]]
  - `06 Three Pillars`: ![[Research/assets/logbook-social-guardian-legacy-06-three-pillars-2026-06-05.png]]
  - `07 Legacy Letter`: ![[Research/assets/logbook-social-guardian-legacy-07-legacy-letter-2026-06-05.png]]
  - `08 Claim Card`: ![[Research/assets/logbook-social-guardian-legacy-08-claim-card-2026-06-05.png]]
  - `09 Quiet Completionist`: ![[Research/assets/logbook-social-guardian-legacy-09-quiet-completionist-2026-06-05.png]]
  - `10 Fireteam Memory`: ![[Research/assets/logbook-social-guardian-legacy-10-fireteam-memory-2026-06-05.png]]

Guardian Legacy rev2 endgame-focused mock pass:
- Contact sheet: ![[Research/assets/logbook-social-guardian-legacy-rev2-contact-sheet-2026-06-05.jpg]]
- This pass adds more useful endgame information while staying close to the quieter `01 Legacy Receipt` / `02 Era Passport` visual family.
- Candidate facts used: Guardian Rank, years active, seals, raid clears, dungeon clears, legendary campaigns, and Monument progress where the layout can support it.
- Rev2 directions:
  - `01 Endgame Dossier`: ![[Research/assets/logbook-social-guardian-legacy-rev2-01-endgame-dossier-2026-06-05.png]]
  - `02 Endgame Passport`: ![[Research/assets/logbook-social-guardian-legacy-rev2-02-endgame-passport-2026-06-05.png]]
  - `03 Endgame Arc`: ![[Research/assets/logbook-social-guardian-legacy-rev2-03-endgame-arc-2026-06-05.png]]
- Production note: ImageGen can only approximate the logo treatment in concept mocks. Production cards and coded mocks should place the real Warmind/Charlemagne logo asset deterministically, using blended/textured styling if desired without changing the mark.

Session Recap mock pass:
- Contact sheet: ![[Research/assets/logbook-social-session-recap-contact-sheet-2026-06-05.jpg]]
- This pass keeps to fields that can come from static session-card generation: session title/subtitle, player, started/ended time, active time, activity count, primary mode family, activity mix, achievement count/chips, commend count, and conservative played-with counts.
- Mocked route text uses `warmind.io/p/{guid}/s/{sessionId}`.
- Directions generated for iteration:
  - `01 Endgame Dossier`: ![[Research/assets/logbook-social-session-recap-01-endgame-dossier-2026-06-05.png]]
  - `02 First Clear Stamp`: ![[Research/assets/logbook-social-session-recap-02-first-clear-stamp-2026-06-05.png]]
  - `03 Balanced Mix`: ![[Research/assets/logbook-social-session-recap-03-balanced-mix-2026-06-05.png]]
  - `04 Commended Session`: ![[Research/assets/logbook-social-session-recap-04-commended-session-2026-06-05.png]]
  - `05 Quiet Session`: ![[Research/assets/logbook-social-session-recap-05-quiet-session-2026-06-05.png]]
  - `06 Crucible Streak`: ![[Research/assets/logbook-social-session-recap-06-crucible-streak-2026-06-05.png]]
  - `07 Gambit Set`: ![[Research/assets/logbook-social-session-recap-07-gambit-set-2026-06-05.png]]
  - `08 Marathon Session`: ![[Research/assets/logbook-social-session-recap-08-marathon-session-2026-06-05.png]]
  - `09 Return To Orbit`: ![[Research/assets/logbook-social-session-recap-09-return-to-orbit-2026-06-05.png]]
  - `10 First Clears Session`: ![[Research/assets/logbook-social-session-recap-10-first-clears-session-2026-06-05.png]]

PGCR Receipt mock pass:
- Contact sheet: ![[Research/assets/logbook-social-pgcr-receipt-contact-sheet-2026-06-05.jpg]]
- This pass keeps to fields that can come from static PGCR share generation: instance ID, period/date, activity hash/name, mode/mode name, duration, completed/outcome, player, kills, deaths, assists, K/D, fireteam size, and share path.
- Mocked route text uses `warmind.io/p/{guid}/pgcr/{instanceId}` to match the current share-path shape. Final canonical path can still be revisited.
- Directions generated for iteration:
  - `01 Raid Clear`: ![[Research/assets/logbook-social-pgcr-receipt-01-raid-clear-2026-06-05.png]]
  - `02 Crucible Win`: ![[Research/assets/logbook-social-pgcr-receipt-02-crucible-win-2026-06-05.png]]
  - `03 Solo Dungeon`: ![[Research/assets/logbook-social-pgcr-receipt-03-solo-dungeon-2026-06-05.png]]
  - `04 Nightfall Completion`: ![[Research/assets/logbook-social-pgcr-receipt-04-nightfall-completion-2026-06-05.png]]
  - `05 Gambit Victory`: ![[Research/assets/logbook-social-pgcr-receipt-05-gambit-victory-2026-06-05.png]]
  - `06 Trials Match`: ![[Research/assets/logbook-social-pgcr-receipt-06-trials-match-2026-06-05.png]]
  - `07 Raid Endurance`: ![[Research/assets/logbook-social-pgcr-receipt-07-raid-endurance-2026-06-05.png]]
  - `08 Close Match`: ![[Research/assets/logbook-social-pgcr-receipt-08-close-match-2026-06-05.png]]
  - `09 Story Mission`: ![[Research/assets/logbook-social-pgcr-receipt-09-story-mission-2026-06-05.png]]
  - `10 Compact Minimal`: ![[Research/assets/logbook-social-pgcr-receipt-10-compact-minimal-2026-06-05.png]]

Current unfurl mock pass:
- Contact sheet: ![[Research/assets/logbook-social-unfurl-contact-sheet-2026-06-05.jpg]]
- Stage 1 priority target mocks:
  - Discord: ![[Research/assets/logbook-social-unfurl-01-discord-2026-06-05.png]]
  - X: ![[Research/assets/logbook-social-unfurl-02-x-2026-06-05.png]]
  - Bluesky: ![[Research/assets/logbook-social-unfurl-03-bluesky-2026-06-05.png]]

Open privacy and product questions:
- Confirm publish states for profile/session/PGCR share URLs: private, unlisted, public, and whether registered/non-registered targets differ.
- Define unpublish/delete/cache purge behavior before broad public sharing.
- Decide whether shared PGCR cards can show other players' names, clan context, or stat lines without explicit Charlemagne consent.
- Keep comments and user-generated text out of share metadata/cards until moderation and sanitization are designed.
- Add share analytics without incentivizing spam.

Remaining ImageGen queue:
- Historical note: ImageGen was persistently rate-limited earlier on 2026-06-05 while OpenAI status was reporting account/subscription/credit-impact issues. The Guardian Legacy and Stage 1 unfurl mock passes resumed successfully later the same day.
- Remaining detail-page mocks to generate:
  - Timeline detail page: chronological night-in-order PGCR timeline with inline expansion.
  - PvP-forward detail page: PvP session summary and mode breakouts are the main organizing element when a session is Crucible-heavy.
  - Raid/story-forward detail page: primary raid/dungeon receipt gets a stronger visual treatment while supporting activities stay lower priority.
- Share-card mocks to generate:
  - Refine selected Session Recap and PGCR Receipt directions after product/design review.

### Milestone 4: Profile Subpages
- Add profile subpages for deeper category views such as raids, crucible, collections, and similar sections.

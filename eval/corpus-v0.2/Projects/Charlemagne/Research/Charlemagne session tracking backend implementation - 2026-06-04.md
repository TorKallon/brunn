Created: 2026-06-04
Updated: 2026-06-04
Status: Backend merged; private web UI next
Related: [[Projects/Charlemagne/Charlemagne|Charlemagne]], [[Projects/Warmind/Warmind|Warmind]], [[Projects/Charlemagne/Research/Charlemagne Destiny activity social network plan - 2026-06-02|Destiny activity social network plan]]

# Charlemagne session tracking backend implementation - 2026-06-04

## Summary

The session tracking backend has been implemented and merged into Warmind. This turns committed Destiny 2 PGCRs for Charlemagne users into private play sessions, compact activity events, session summaries, and session achievements.

The next product step is mostly UI: build a private, web-first session summaries experience before considering notifications, Discord recaps, public feeds, or social sharing.

## Repo state

- PR: https://github.com/warmind-io/warmind/pull/36
- Session tracking merge commit: `df013d85b5572014364536537f921f68a2cd90b0`
- Current checked remote tip after later follow-up work: `origin/master` includes session tracking and was observed at `f105abd7512e627a1351cb9d78d01108d7aee7c7`.
- Repo-local plan: `/Users/Shared/projects/warmind/docs/specs/session-tracking-backend-plan.md`

## What shipped

- Parser impact is limited to one post-transaction enqueue call in `parser/workers.go` after PGCR commit.
- Parser-side helper uses the existing Charlemagne membership map and enqueues only PGCR pointers for non-zero Charlemagne users.
- Cortex owns PGCR retrieval, filtering, event projection, sessionization, finalization, achievement checks, and recovery.
- New product tables live in `charlemagne`, not `nexus`, because the feature is scoped to Charlemagne users.
- Compact event projection is stored in `charlemagne.session_activity_events`.
- Session rows are stored in `charlemagne.play_sessions`.
- PGCR idempotency and recovery state are stored in `charlemagne.session_pgcr_ingest`.
- Achievement state is stored in `charlemagne.session_achievement_state`.
- Generated achievements are embedded on `play_sessions.achievementsJson` for the first milestone.
- Session summaries are embedded on `play_sessions.summaryJson`.
- Session grouping uses one semantic knob: `session_gap`, implemented with a default of one hour.
- Finalization uses claims and revision checks so late PGCRs can reopen/recalculate a session without stale writes winning.
- Recovery uses committed PGCR truth rather than trusting Redis/gocraft delivery as durable.

## API surface

Authenticated SPA routes exist under the `/spa` route group:

- `GET /spa/sessions`
- `GET /spa/sessions/:sessionId`

The list route returns recent user-owned sessions. The detail route returns the requested user-owned session plus materialized summary, achievements, and activity events. Reads should stay on the session tables; UI request paths should not query raw PGCR JSON, weekly summaries, or large historical activity tables.

## Achievement scope

Initial checks are intentionally simple and pluggable:

- longest active play session this calendar year;
- best completed-match Crucible K/D this calendar year;
- most Crucible wins in one session this calendar year;
- first completion of a rolled-up raid this season;
- first ever completion of a rolled-up raid;
- first ever full clear of a rolled-up raid;
- fastest full completion of a rolled-up raid;
- new raid carries added during the session.

Achievement checks run from a registry during session close. Each check has a clear boundary: inspect the session/events and tiny achievement state, return achievements and state updates, and fail closed without breaking session finalization.

## Validation

Focused tests passed during the implementation:

- `go test ./sessiontracking ./sweeperbot ./parser ./warmindapi ./charlemagne -count=1`
- `go test ./... -vet=off -count=1`

Nyx catch-up canary passed with parser catch-up behavior:

- replayed 3000 PGCRs over a six-plus-hour window;
- `raw_missing=0`;
- ingest processed/ignored rows were produced correctly;
- ready sessions were generated;
- expired open sessions cleared on finalizer passes;
- covered PvE, PvP, raid, and dungeon mode families;
- no social events started sessions;
- no invalid session summary, achievement, event stat, or achievement state JSON was observed.

A later Warmind soak handoff also recorded live session tracking progress while the parser was catching up. That note lives under Warmind validation context rather than this Charlemagne product note.

## Known caveats

- Normal `go test ./... -count=1` on clean `origin/master` was blocked by pre-existing vet warnings outside the session tracking work. The vet-off broad suite passed.
- The canary volume was enough to validate session mechanics, but broader live soak remains useful before a user-visible launch.
- Achievement mechanics were covered structurally; real award quality should be watched with live data before surfacing them too loudly.
- Notifications, Discord delivery, public social surfaces, kudos, comments, and server recaps remain deferred.

## UI next

The next step is a private SPA experience for session summaries.

Suggested first UI slice:

- recent sessions page for the logged-in Charlemagne user;
- session detail page using `GET /spa/sessions/:sessionId`;
- compact session header with start/end, active time, activity count, primary mode family, and mode mix;
- achievement highlights from `achievements`;
- activity timeline with PGCR/activity links and mode-family badges;
- summary totals for kills, deaths, assists, wins, and completed activities where present;
- empty, loading, error, and not-authorized states;
- a quiet "private by default" posture, without social or notification controls yet.

Before broad exposure, do one more live soak/checkpoint and confirm:

- parser enqueue remains non-fatal;
- recovery has no stale `missing_pgcr` markers;
- expired open sessions converge to `ready`;
- no invalid JSON is produced;
- API cannot read another user's session.

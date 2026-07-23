Created: 2026-05-18
Updated: 2026-05-18
Status: Reference

Related: [[N24 RaceWatch]], [[2026-05-18 Offseason and NLS6 handoff]], [[2027 Endurance Calendar and Timing Research]]

# 2026-05-18 FastN24 competitor note

FastN24 is a community-built F1-style 24h Nurburgring / NLS timing dashboard found through Reddit.

Sources:

- Reddit post: https://www.reddit.com/r/wec/comments/1tex2yb/got_tired_of_the_official_24h_n%C3%BCrburgring_live/
- Live app: https://24h-wec-nurburgring.vercel.app/

## Why it matters

This is both a competitor and a useful signal. It validates that fans are frustrated with the official Azure-hosted live timing UI and want a richer N24/NLS timing layer with class context, sector data, race-control state, and lap history.

RaceWatch should not copy it. RaceWatch should keep its own identity: mobile-friendly, plain-English race context, source/caveat discipline, and AI-generated race insights. But FastN24 shows that there is real demand for deeper timing surfaces alongside the story layer.

## Reported features

- All nine Nordschleife sector/intermediate columns.
- Personal-best green and overall-best purple timing cues.
- Class-colored rows with full class labels and in-class rank.
- Race-control panel with flag categorization and de-duplication by ID.
- Lap chart colored against each car's own median pace.
- Per-car hover card with same-team cross-reference and recent laps.
- PRO badge, pit indicator, gap, and interval.

## Observed technical shape

The public app bundle indicates:

- Direct browser WebSocket to `wss://livetiming.azurewebsites.net/`.
- Default event id `50`.
- Subscription PIDs include `0`, `3`, `4`, `7`, and `9002`.
- Active rule polling uses `https://api-racingios.gpsoverip.de/v1/racing/rules/active?overipapp=IPADIPHADAC24H`.
- Views include timing, lap chart, and track/map.
- It has an "Awaiting first snapshot" state when the live feed has not produced data.

The author notes that browser lap history is not backfilled yet; it only sees laps completed after the browser opens. The author says a CLI capture tool can pull cumulative history through another WebSocket subscription, but that is not wired into the browser.

## Competitive read

Strengths:

- Dense desktop power-user timing view.
- Better class and sector visibility than the official timing page.
- Race-control visibility.
- Useful car detail/hover affordances.
- Lap chart makes pace history more scannable than a raw table.

Weak spots or risks:

- Direct browser upstream connections may run into regional blocks, browser failures, or excessive upstream load.
- No server-side fanout/backfill in the public browser path yet.
- The dashboard appears more timing-expert-oriented than beginner/mobile-oriented.
- It does not replace RaceWatch's plain-English insight/story role.

Community feedback worth remembering:

- Users asked for Code 60 zone visibility.
- Some users saw "Awaiting first snapshot" and discussed VPN/regional access issues.
- A commenter suggested proxying the data so one upstream connection serves many users.

## RaceWatch feature inspiration

High-value ideas for RaceWatch:

- Advanced timing view with all nine intermediates, class rank, class labels, personal best, and official/class purple signals.
- De-duplicated race-control panel with red/amber/green/neutral grouping.
- Code 60 / double-yellow zone surface if the active-rules endpoint proves stable.
- Lap history and median-relative pace heatmap using persisted RaceWatch capture data, not client-only history.
- Tap-friendly car detail drawer with recent laps, team-mate entries, stint age, pit count, class rank, and gap context.
- Later: a simple track/map view for focused cars and slow zones.

Suggested priority before NLS6:

- Verify regular NLS event id and PID availability, especially PID 7 and PID 9002.
- Improve class label/class rank visibility in RaceWatch's live leaderboard.
- Prototype race-control / Code 60 ingestion with source freshness and de-duplication.
- Add a car detail drawer before attempting a full power-user timing grid.

Guardrails:

- Do not copy FastN24 design, screenshots, code, naming, or exact interaction patterns.
- Keep RaceWatch's primary surface mobile-first and explanatory.
- Keep official facts, inferred strategy, and caveats visibly separate.
- Prefer RaceWatch's Cloudflare/server fanout and persisted history over direct client-to-official timing connections.

Created: 2026-05-13 16:30 PDT
Updated: 2026-05-18
Status: Complete
Health: Race run complete

Related: [[Active projects]], [[INDEX|Shared knowledge index]], [[Home]], [[Topics/F1/F1|F1]], [[Projects/Treehouse/Treehouse|Treehouse]]

## Purpose
N24 RaceWatch is an English-first Nürburgring 24h companion for F1 and Max Verstappen fans. It translates official timing and race-state signals into plain-English context instead of trying to replace official timing.

## Current status
- Public prototype is live at https://n24racewatch.com/.
- Frontend is a Vite + React + TypeScript app deployed through Vercel.
- Canonical domain is `n24racewatch.com`, with `www` redirecting to the apex.
- Core views include Max Watch, Race Overview, and Explainer.
- Current app has a production realtime leaderboard path plus deterministic simulator scenarios, health/snapshot artifacts, and production QA coverage.

## Current focus
The 2026 N24 race run is complete. RaceWatch is now in an off-season / between-races state, with the public homepage counting down to NLS6 on 20 June 2026 and preserving the 2026 race archive, screenshots, rules explainers, and production learning.

## Next step
Pick this back up shortly before NLS6 using [[2026-05-18 Offseason and NLS6 handoff]], [[2026 Production Wrap-up]], and the repo playbooks:

- `/Users/shared/projects/n24-racewatch/docs/2026-production-wrap-up.md`
- `/Users/shared/projects/n24-racewatch/docs/race-day-operations-playbook.md`
- `/Users/shared/projects/n24-racewatch/docs/post-race-shutdown-playbook.md`
- `/Users/shared/projects/n24-racewatch/docs/nyx-development.md`

## Repo and routing
- Canonical local repo: `/Users/shared/projects/n24-racewatch`
- Nyx dev preview URL: `http://nyx:5173/` when `n24-offseason-dev` or another N24 frontend dev session is running.
- Older OpenClaw checkout: `/Users/aether/.openclaw/workspace/n24-racewatch` is historical/dirty; do not use it as the runnable source of truth.
- GitHub: `git@github.com:TorKallon/n24-racewatch.git`
- Public URL: https://n24racewatch.com/
- Vercel project: `n24-racewatch`
- Related docs in repo: `README.md`, `docs/build-brief.md`, `docs/domain-seo-promo.md`

## Key links
- [[Active projects]]
- [[Home]]
- [[INDEX|Shared knowledge index]]
- [[Topics/F1/F1|F1]]
- [[Projects/Treehouse/Treehouse|Treehouse]]
- [[2026-05-18 Offseason and NLS6 handoff]]
- [[2026-05-18 FastN24 competitor note]]
- [[2026 Production Wrap-up]]
- [[2027 Endurance Calendar and Timing Research]]

## Notes
Keep the vault note lightweight. Detailed implementation truth lives in the repo, current production behavior, and active QA/scheduler notes.

- 2026-05-16 production timing chart update: Race Overview timing rows show best lap and last lap instead of derived pace. Current driver is sourced from official timing `NAME` only when a separate `TEAM` field is present. Tire/tyre compound is not currently present in the official row payload, so the UI hides tire and keeps stint data when available. Invalid interval strings such as `R001` are filtered before display.
- 2026-05-16 live gap semantics: During the race, official timing `GAP` can switch meaning by lap group. `----LAP 39` marks the first car in a lap group, and later same-lap rows can report gap to that group rather than to P1. RaceWatch should display `Gap P1` as a same-lap numeric gap only when the car is on the leader lap, otherwise as `+N lap(s)` from official lap counts.
- 2026-05-16 Codex/Nyx live ops: Current production truth is realtime-first. Cloudflare Worker Durable Object serves `https://n24racewatch.com/api/n24-racewatch/realtime/latest` and `/realtime/ws`; Nyx publishes slower strategy/timeline context to `https://n24racewatch.com/api/n24-racewatch/insights`. Heartbeat command is `cd /Users/shared/projects/n24-racewatch && npm run ops:heartbeat`. It manages the `n24-insights` tmux publisher, checks realtime health/latest plus Blob freshness, and repairs the Blob publish once when realtime latest has rows but the active insight feed does not.
- 2026-05-16 Codex heartbeat runner: race-day ops should keep `n24-insights` and `n24-heartbeat` tmux sessions running from `/Users/shared/projects/n24-racewatch`. `npm run ops:start-heartbeat` starts a 45-second JSON heartbeat loop that writes `.local-data/n24-racewatch/ops/heartbeat.log` and keeps the frontend insight/leaderboard path supervised without re-enabling OpenClaw cron jobs.
- 2026-05-16 Codex-derived insights split feed: OpenClaw cron jobs remain disabled. Nyx now runs a separate `n24-codex-insights` tmux heartbeat via `npm run ops:start-codex-insights`, default 180 seconds. It uses `codex exec` locally, not the app/OpenAI API, reads production official timing/context plus realtime latest, validates source-grounded Codex strategy/timeline output, and publishes to `https://n24racewatch.com/api/n24-racewatch/codex-insights`. The frontend overlays only Codex-badged strategy/timeline items on top of the official realtime feed. Public copy should say "latest timing" or "source data", never internal "packet" language.
- 2026-05-16 production verification: Vercel production deployment `dpl_FX4Yc1M6jJzVhmyEsyTWWfhQ1atD` is aliased to `https://n24racewatch.com`. Headless Chrome verified the latest Codex heartbeat card `Winward 1-2 Is A Real Road Fight`, `CODEX INSIGHT` chip, race-intelligence timeline item, live timing rows for #3/Auer, and no public `packet` wording. Ops status was green for `n24-insights`, `n24-heartbeat`, `n24-codex-insights`, realtime health/latest, official Blob, and Codex Blob.
- 2026-05-16 final live-insight verification: After restarting `n24-codex-insights` from the current script, the loop published `#3 is in the lead fight on matched pit counts` at `2026-05-16T20:58:02Z`. Production `/api/n24-racewatch/codex-insights` returned `generatedBy: codex-heartbeat`, no public `packet` wording, and the frontend rendered the `CODEX INSIGHT` chip, that card, live timing, and #3/Winward driver context.
- 2026-05-16 endurance racecraft note: Same-team or same-manufacturer lead swaps can be a legitimate endurance-racing management pattern, but RaceWatch should publish this only as a caveated timing read after it persists. Background examples include Toyota's Portimao 2021 same-team position swaps around traffic/pace sectors and different fuel strategies (Motorsport.com), Toyota's Le Mans 2018 TS050s swapping places several times (Toyota pressroom), Le Mans rules where refuelling/FCY/pit access are strategic variables (ACO), and N24 red-flag procedure where stint-length differences can matter. Public copy must not claim confirmed team orders, fuel saving, stint extension, or manufacturer intent unless an official/source detail confirms it.
- 2026-05-16 Max night stint context: During the race, official timing listed Verstappen in #3 after the ninth recorded stop and later P1/C1. RaceWatch used an expiring local `codex-operator-context.json` note to steer the Codex heartbeat toward a Max night-stint card while keeping current facts from official timing. Public wording should frame this as a first real 24-hour race night chapter/race-night stint, not as first-ever dark laps; qualifying coverage indicated he had already run dark laps earlier in the week.
- 2026-05-16 live replay capture: Added `scripts/n24-capture-live.mjs` and `npm run ops:capture-live` to capture raw official timing websocket traffic, production realtime websocket snapshots, and periodic HTTP API snapshots into `.local-data/n24-racewatch/captures/`. Active capture `2026-05-17T00-45-16Z-race-live-window` started during Verstappen's night stint and is intended for future replay fixtures, promotional screenshots, and websocket behavior testing. Do not commit raw `.local-data` captures by default; extract small sanitized fixtures later if needed.
- 2026-05-16/17 concurrent websocket load: [[Concurrent websocket load 2026-05-16]] records 282 health-endpoint samples from 16:05 PDT to 08:47 PDT. Time-weighted average was 39.5 connected websocket clients, with a high point of 60 at 2026-05-17 03:07 PDT.
- 2026-05-17 post-race wrap-up: RaceWatch production monitoring is stopped. N24 OpenClaw active/config crumbs were removed. Durable docs now live in the shared repo: `docs/2026-production-wrap-up.md`, `docs/race-day-operations-playbook.md`, and `docs/post-race-shutdown-playbook.md`. Retained data includes the local live capture, committed Max night-stint keyframes, and committed desktop/mobile promo screenshots.
- 2026-05-17 calendar/timing research: [[2027 Endurance Calendar and Timing Research]] records announced 2027 N24/Qualifiers dates, the currently missing full 2027 NLS calendar, and a first-pass comparison of Spa, Le Mans/WEC, IMSA, Bathurst, IGTC, and 24H Series timing-provider compatibility. Direct RaceWatch live-ingest reuse looks most promising for NLS / 24h Qualifiers; other endurance races likely need provider-specific adapters.
- 2026-05-18 off-season homepage: Production commit `2895b2c` made the off-season homepage live with RaceWatch positioning, 2026 archive links, production screenshots, rules explainers, and 2027 countdowns. Production commit `f101e5b` corrected the next covered event to 2026 NLS6 / 1. ADAC Eifel Trophy on 20 June 2026 and added the remaining 2026 NLS schedule. Vercel deployed it via Git-backed production deployment with trusted commit author `Aether (TorKallon automation) <217879+TorKallon@users.noreply.github.com>`.
- 2026-05-18 Treehouse card: `/Users/Shared/projects/treehouse` commit `8a5b742` refreshed the N24 RaceWatch project card to explain WebSocket live timing, AI-generated race insights, and F1-fan-friendly endurance-rules translation.
- 2026-05-18 SEO pass: A local SEO pass was prepared for the off-season site but not yet committed or deployed. It updates static metadata, route-aware titles/descriptions, sitemap/robots/manifest/OG text, JSON-LD for the site/app/NLS6 event, and public copy around Nurburgring/NLS live timing, RaceWatch insights, F1-fan explainers, class gaps, pit cycles, and Code 60 context. See [[2026-05-18 Offseason and NLS6 handoff]].
- 2026-05-18 FastN24 competitor/inspiration note: [[2026-05-18 FastN24 competitor note]] captures a community-built F1-style N24/NLS timing dashboard at `24h-wec-nurburgring.vercel.app`. Treat it as competitor and product signal: advanced sector/class timing, race-control panel, lap chart, car hover cards, Code 60/active-rule ideas, and the need for server-side fanout/backfill rather than every browser opening its own upstream timing connection.

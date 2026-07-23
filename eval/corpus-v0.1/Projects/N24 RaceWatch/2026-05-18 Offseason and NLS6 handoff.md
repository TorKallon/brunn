Created: 2026-05-18
Updated: 2026-05-18
Status: Handoff

Related: [[N24 RaceWatch]], [[2026 Production Wrap-up]], [[2027 Endurance Calendar and Timing Research]], [[2026-05-18 FastN24 competitor note]], [[Projects/Treehouse/Treehouse|Treehouse]]

# 2026-05-18 Offseason and NLS6 handoff

RaceWatch is now in an off-season / between-races state after the 2026 N24 production run.

## Current public state

- Public site: https://n24racewatch.com/
- Canonical repo: `/Users/shared/projects/n24-racewatch`
- GitHub: `git@github.com:TorKallon/n24-racewatch.git`
- Vercel project: `n24-racewatch`
- Current production commit: `f101e5b` (`Add 2026 NLS schedule countdown`)
- Previous off-season homepage commit: `2895b2c` (`Add off-season RaceWatch homepage`)
- Treehouse card update commit: `/Users/Shared/projects/treehouse` commit `8a5b742` (`Refresh N24 RaceWatch project card`)

The production homepage is no longer the live race dashboard by default. It is an off-season homepage that explains RaceWatch, shows 2026 production screenshots, links to the 2026 archive and live view, explains the relevant N24/NLS rules in plain English, and counts down to the next covered event.

## Next event

Next covered event:

- NLS6 / 1. ADAC Eifel Trophy
- Date: 20 June 2026
- Current homepage primary countdown points here.

Remaining 2026 NLS schedule shown on the site:

| Date | Slot | Event |
| --- | --- | --- |
| 20 June 2026 | NLS6 | 1. ADAC Eifel Trophy |
| 1 August 2026 | NLS7 | KW 6h ADAC Ruhr-Pokal-Rennen |
| 12 September 2026 | NLS8 | 65. ADAC Reinoldus-Langstreckenrennen |
| 13 September 2026 | NLS9 | 58. ADAC Barbarossapreis |
| 10 October 2026 | NLS10 | 2. NLS Sportwarte-Trophy |

The first five 2026 race slots are shown as past context:

- 14 March 2026: NLS1 / 71. ADAC Westfalenfahrt
- 21 March 2026: NLS2 / 58. ADAC Barbarossapreis
- 11 April 2026: NLS3 / 57. Adenauer ADAC Rundstrecken-Trophy
- 18-19 April 2026: ADAC 24h Qualifiers as NLS4+5

## What to do when picking this up before NLS6

Use Nyx dev first:

```bash
cd /Users/shared/projects/n24-racewatch
git status --short --branch
git pull --ff-only
npm test
npm run dev -- --host 0.0.0.0 --port 5173 --strictPort
```

Human dev URL from Erebus:

```text
http://nyx:5173/
```

Before NLS6, verify:

- Official NLS live timing still uses the expected WIGE/Azure timing family.
- The regular NLS event ID and PID subscription shape are confirmed for 2026 NLS6. N24/Qualifiers used event id 50; regular NLS has historically used a different id, often 20, so do not assume without probing.
- The old race-day backend/ops runners are still stopped until intentionally restarted.
- OpenClaw crons remain out of the production path.
- Vercel Git deployment identity remains `Aether (TorKallon automation) <217879+TorKallon@users.noreply.github.com>`.
- Production deploys come from committed, pushed `main`, not dirty local Vercel CLI state.

## Race-day playbook pointers

Durable repo docs:

- `/Users/shared/projects/n24-racewatch/docs/2026-production-wrap-up.md`
- `/Users/shared/projects/n24-racewatch/docs/race-day-operations-playbook.md`
- `/Users/shared/projects/n24-racewatch/docs/post-race-shutdown-playbook.md`
- `/Users/shared/projects/n24-racewatch/docs/live-captures.md`
- `/Users/shared/projects/n24-racewatch/docs/nyx-development.md`

Useful race-day commands live in the repo playbook. Start from:

```bash
cd /Users/shared/projects/n24-racewatch
node scripts/n24-ops.mjs status --json
node scripts/n24-watch.mjs once --json
```

Only start live loops when a real event window is near:

```bash
npm run ops:start-live
npm run ops:start-heartbeat
npm run ops:start-codex-insights
```

Stop them promptly after an event so Codex/runtime loops do not keep waking.

## Content and product state

Homepage positioning:

- RaceWatch translates official Nürburgring endurance timing into a clear race story.
- The site is built for mobile, real-time timing, class/gap context, slow-zone explanations, and race insights.
- Public copy should speak to fans, not to internal infrastructure.
- Avoid public terms like "packet"; say "timing", "official timing", "latest timing", or "source data".

Archive state:

- `/archive/2026` shows what the 2026 N24 production site looked like when live.
- `/live` still routes to the live RaceWatch app shell.
- Production screenshots live under `docs/promo/screenshots/2026-05-17T02-44-47-298Z-live-production/`.
- Portable replay fixture lives at `testdata/live-captures/2026-05-17-max-night-stint-keyframes.json`.

Treehouse:

- The Rourke/Treehouse project card was updated to describe RaceWatch as WebSocket live timing plus AI-generated race insights for F1 fans learning endurance-racing rules.

## SEO state

A local SEO pass was prepared after the NLS6 production update. It has not been committed, pushed, or deployed unless a later note says otherwise.

Prepared repo changes:

- Static `index.html` metadata now targets RaceWatch, Nurburgring/Nuerburgring 24h, NLS, live timing, race insights, and F1-fan explainers.
- Route-aware browser metadata was added for `/`, `/archive/2026`, and `/live`.
- `sitemap.xml`, `robots.txt`, the web manifest, and the OG card were updated.
- JSON-LD now includes WebSite, WebApplication, and a SportsEvent entry for NLS6 / 1. ADAC Eifel Trophy on 20 June 2026.
- The homepage and archive copy were lightly adjusted so the public positioning includes NLS, real-time mobile timing, race insights, class gaps, pit cycles, and Code 60 context.

Suggested future SEO/product architecture:

- Add dedicated crawlable pages for NLS live timing, the 2026 NLS schedule, Nurburgring 24h live timing explained, Code 60 explained, and Nurburgring endurance rules for F1 fans.
- Move the rules explainers into first-class pages with FAQ-style structure and FAQPage schema.
- Consider static prerendering or a Next/Astro-style marketing layer so route-specific content and metadata exist in HTML without relying on the Vite SPA runtime.

## Research notes

Use [[2027 Endurance Calendar and Timing Research]] for the NLS/N24 calendar, timing-stack compatibility, and rule-family analysis.

Use [[2026-05-18 FastN24 competitor note]] before the next live UI planning pass. It captures a community F1-style N24/NLS timing dashboard as both competitor and inspiration for advanced timing, race-control, Code 60, lap-chart, and car-detail features.

Important current conclusion:

- Direct RaceWatch live-ingest reuse is strongest for NLS, ADAC 24h Qualifiers, and N24 because they are the same Nordschleife/Nürburgring timing/rule family.
- Spa, Le Mans/WEC, IMSA, Bathurst, IGTC, and 24H Series may be good future RaceWatch-style products, but they should not be treated as plug-and-play with the current WIGE/Azure collector.

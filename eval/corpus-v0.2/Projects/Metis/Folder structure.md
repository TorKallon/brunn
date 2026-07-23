Created: 2026-04-18 11:58 PDT
Updated: 2026-04-18 11:58 PDT

## Goal
Define a top-level vault structure that matches the real jobs Metis needs to do.

## Design principles
- optimize for retrieval and operations, not taxonomy purity
- keep high-sensitivity records easy to find but clearly separated
- give projects and trips dedicated homes
- let research stay messy at first
- let evergreen pages emerge from repeated use

## Proposed top-level folders
- `Inbox/`
- `Journal/`
- `Projects/`
- `Trips/`
- `Areas/`
- `Records/`
- `Topics/`
- `Research/`
- `Briefings/`
- `Archive/`
- `Assets/` (optional, only if needed)

## Root-level navigation notes
At the vault root, keep a very small number of high-value jumping-off notes:
- `Home.md`
- `Active projects.md`
- optionally one or two other pinned notes later

These are part of the main UX.
They matter more than making the folder tree pretty.

## What each folder is for
### `Inbox/`
Fast capture, temporary holding, uncategorized notes.
Nothing should live here forever.

### `Journal/`
Daily and dated notes, check-ins, logs, trip-day notes, and short-lived operational context.

### `Projects/`
Finite efforts with active work and explicit outcomes.
Examples:
- Metis
- Joyeuse
- Ithrion
- Charlemagne initiatives

Each project should have a stable overview note with lightweight status fields so project portfolio questions can be answered easily.

### `Trips/`
Vacations, ski weekends, race travel, weekend trips, and other travel workspaces.
This deserves its own top-level home because it mixes planning, documents, logistics, calendar coordination, and live daily use while traveling.

### `Areas/`
Ongoing life domains without a clean end date.
Examples:
- Family
- Home
- Finance
- Health
- Skiing
- Work

This is where summary and operating pages for finance/health should live.

### `Records/`
Sensitive or source-oriented documents and extracted document records.
Suggested subfolders:
- `Records/Finance/`
- `Records/Health/`
- `Records/Household/`
- `Records/Identity/`
- `Records/Insurance/`
- `Records/Legal/`
- `Records/Taxes/`

This is the retrieval layer, not the main thinking layer.

### `Topics/`
Evergreen knowledge pages for recurring subjects.
Examples:
- ski tuning
- family travel preferences
- health topics
- vendor/account reference pages

### `Research/`
Exploratory notes, thought experiments, investigations, and one-off deep dives.
Anything that becomes important can later graduate into `Projects/`, `Areas/`, or `Topics/`.

### `Briefings/`
Generated or curated briefings, updates, summaries, and standing reports.

### `Archive/`
Inactive or superseded notes, old structures, imported material kept mainly for reference.

### `Assets/`
Attachments, images, exports, or OCR sidecars if we decide not to store raw OCR inline.
Only use if it genuinely reduces clutter.

## Key architectural decision
Finance and health should each exist in two layers:
- `Areas/Finance/` and `Areas/Health/` for ongoing summaries, planning, and status
- `Records/Finance/` and `Records/Health/` for source documents and extracted records

That preserves both usability and trust.

## Household planning nuance
Household planning exists, but is not the dominant organizing force.
It should mostly live under:
- `Areas/Home/` for ongoing household operations
- `Trips/` for weekend trips and travel planning

## Recommended substructure patterns
### Projects
Each project can have:
- `Overview.md`
- `Plan.md`
- `Worklog.md`
- `Decisions.md`
- `Open questions.md`
- `References/`

### Trips
Each trip can have:
- `Overview.md`
- `Itinerary.md`
- `Bookings.md`
- `Packing or prep.md`
- `Daily notes/`
- `Source docs/`

### Areas
Each area should have a stable summary page and optional supporting notes.

### Records
Prefer one note per important document, plus source link and OCR/extracted data.

## Why this structure fits
This structure cleanly supports:
- sensitive retrieval
- finance and health history
- trip planning and live travel use
- active project working memory
- exploratory research without over-organization

## Recommendation
Use this as the target live structure.
Create folders gradually, but design templates and rules against this model now.
Remember that folders are backend scaffolding, while `Home`, search, bookmarks, and assistant retrieval are the real front door.

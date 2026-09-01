# Interim Briefing Rewire Contract

Status: ready to apply, 2026-08-01

Purpose: resume the 6:30 morning briefing immediately by replacing every
retired local path (Obsidian vault, `~/.openclaw/workspace/memory/*`) with
Brunn workspace writes, using only the existing memory tools. No
Brunn repo changes are required. The structured briefings platform
(`docs/superpowers/specs/2026-08-01-briefings-design.md`) later upgrades the
same paths; this contract is forward-compatible with it.

## 1. Paths

| Purpose | Brunn path | Replaces |
| --- | --- | --- |
| Morning briefing note | `Briefings/2026/Morning briefing - <YYYY-MM-DD>.md` | `~/obsidian/notes/Morning briefing - <date>.md` |
| News dedupe state | `Briefings/State/news-brief-state.md` | `~/.openclaw/workspace/memory/news-brief-state.json` |
| Standing trackers | `Briefings/State/daily-trackers.md` | `~/.openclaw/workspace/memory/daily-trackers.md` |
| Research notes (optional, per run) | `Briefings/Research/<YYYY-MM-DD> - <lane>.md` | `sources/Temp/Morning briefing research - ...` scratch files |

There is no `Home.md` Today-link step and no Obsidian sync step. The briefing
is read in the Brunn SPA or via `memory.read`.

## 2. Formats

**Briefing note**: same body style as existing briefings (no frontmatter;
`Created:`/`Updated:` header lines; `# Morning briefing - <date>` H1;
`## 30-second version` first; compact bold-linked news items with "Why this
matters:"). Write with metadata `{"kind": "briefing_edition", "date":
"<YYYY-MM-DD>", "edition": "morning"}`.

**News dedupe state**: Markdown wrapper holding the existing JSON state schema
unchanged in a fenced block, so the current prompt logic ports directly:

````markdown
Updated: <ISO timestamp>

# News brief state

Machine state for morning-briefing dedupe. Do not hand-edit while the cron is
enabled.

```json
{ ...existing news-brief-state.json content... }
```
````

Metadata: `{"kind": "briefing_state"}`. Same wrapper pattern for
`daily-trackers.md` (which stays Markdown as before, plus the metadata kind).

## 3. Rules

1. **Fail closed.** If `memory.open` or any read/write fails, report the
   failure through the normal cron-failure channel and stop. Never write a
   local fallback file, never skip dedupe and send anyway.
2. **Idempotency keys** on every write: `briefing-<date>-morning` for the
   note, `briefing-state-<date>` for state, `briefing-trackers-<date>` for
   trackers. A retried run replays as NoOp instead of duplicating.
3. **Versioned updates.** The 10:00 health job reads the existing note entry
   and rewrites it with the `## Health / fitness` section added or replaced
   and the `Updated:` line bumped (same path — Brunn versioning keeps
   history). It uses idempotency key `briefing-<date>-health-update`.
4. **Dedupe procedure** (ports the current logic): read
   `Briefings/State/news-brief-state.md` plus the three most recent
   `Briefings/2026/Morning briefing - *.md` entries; apply the strict <48h
   underlying-event gate; write the updated state JSON back after publishing.
5. **Privacy unchanged:** no confirmation numbers or private trip details in
   the note; health stays non-diagnostic; no credentials or secret values in
   any workspace write.

## 4. Rewired 6:30 cron prompt

The research pipeline is unchanged (local X/Discord digest scripts, Datadog
snapshots, trackers, stock watchlist). Only the memory/storage steps change.
Merge host-specific script paths from the existing prompt where marked.

```text
Create and deliver Rourke's unified morning briefing for <today's date,
America/Los_Angeles> with strict net-new filtering.

Storage: hosted Brunn is the only memory and delivery store. Begin with
memory.open for this task. If Brunn is unreachable or any read/write
fails, report the failure and stop — do not write local files, do not send an
undeduped briefing.

1. Load state:
   - memory.read "Briefings/State/news-brief-state.md" (JSON state in the
     fenced block) and "Briefings/State/daily-trackers.md".
   - memory.read the three most recent "Briefings/2026/Morning briefing -
     *.md" entries (memory.query title search, lexical).
2. Research (unchanged from the existing prompt): run the local X and Discord
   digest scripts <host-specific paths>; query Datadog for the Charlemagne
   and Joyeuse snapshots; check standing trackers and the stock watchlist;
   gather candidate news per the topic preferences (compact bold-linked
   items, "Why this matters:", absolute dollar moves for stocks, per-stock
   subsections, catalyst check, suppress unchanged tracker baselines).
3. Dedupe: apply the strict <48h underlying-event gate; compare every
   candidate against the state JSON and the three prior briefings; treat
   corroboration without a material delta as already-sent; record omit calls.
4. Compose the note in the established format: Created/Updated header lines,
   "# Morning briefing - <date>" H1, "## 30-second version" first, then
   Intraday updates (if any), then topic sections in the usual order. Include
   "## Health / fitness" only when genuinely fresh early data exists;
   otherwise leave it to the 10:00 job.
5. Publish with memory.write:
   - path "Briefings/2026/Morning briefing - <date>.md", metadata
     {"kind":"briefing_edition","date":"<date>","edition":"morning"},
     idempotency_key "briefing-<date>-morning".
   - updated state JSON to "Briefings/State/news-brief-state.md"
     (idempotency_key "briefing-state-<date>"); updated trackers to
     "Briefings/State/daily-trackers.md" ("briefing-trackers-<date>").
6. Deliver the usual concise iMessage summary with a pointer to the briefing.
7. memory.checkpoint the run: objective, included/omitted decisions, state
   refs.
```

The 10:00 health prompt changes the same way: read the day's note entry,
add/replace `## Health / fitness`, bump `Updated:`, write back with
idempotency key `briefing-<date>-health-update`, send the concise iMessage.

## 5. Applying it

The cron lives on Aether's host; this repo only defines the contract. To
apply: update the two cron prompts per §4, run one supervised catch-up
execution for today, verify the three paths exist in Brunn with expected
content, then re-enable the schedule. The separate stale "MCP readiness"
checker should be updated to probe the current tool names (`memory.open`,
`memory.query`, `memory.read`, `memory.write`, `memory.checkpoint`,
`memory.status`) rather than retired ones.

## 6. Forward compatibility

When the structured platform ships, the cron prompt switches per
`docs/superpowers/specs/2026-08-01-briefings-cron-prompt.md` (Task 15):
`briefing.topics` replaces reading preference files, `briefing.dedupe`
replaces manual state comparison, `briefing.publish` replaces the raw
markdown write. The interim paths remain valid history; the ledger's lexical
near-match lane covers them for dedupe continuity.

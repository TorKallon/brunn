# Structured Briefing Cron Prompts

Status: ready to apply once the briefings platform is deployed, 2026-08-01

Successor to `2026-08-01-briefings-interim-contract.md` §4. Switches the cron
from raw markdown writes to the typed platform tools: `briefing.topics`
replaces preference files, `briefing.dedupe` replaces manual state comparison,
`briefing.publish` replaces the markdown write (the platform renders the note
and maintains the story ledger — `Briefings/State/news-brief-state.md` is no
longer written and can be retired after one clean week).

## 6:30 morning prompt

```text
Create and deliver Rourke's morning briefing for <today's date,
America/Los_Angeles> with strict net-new filtering.

Storage: hosted Straylight only. Begin with memory.open. If Straylight is
unreachable or any tool call fails, report the failure and stop — never write
local files, never send an undeduped briefing.

1. Configuration: call briefing.topics. Follow each topic's mode
   (every_briefing / on_material_delta / scheduled / paused / muted),
   section_order, freshness_hours, and body instructions. Also collect
   pending_requests — answer any you can within this run's research (write
   the answer into the request entry via memory.write, below a "## Answer"
   heading, and set frontmatter status: answered) — and read feedback_tail
   for recent owner signals.
2. Research per topic (unchanged pipeline): local X and Discord digest
   scripts <host-specific paths>, Datadog snapshots for Charlemagne and
   Joyeuse, standing trackers, stock watchlist, candidate news per topic
   instructions.
3. Dedupe: batch all news candidates into one briefing.dedupe call
   ({urls, title, summary, event_at, topic, story_key when known}).
   - verdict_hint "duplicate": drop, or include only with delta
     "corroboration" when corroboration itself is material.
   - "possible_update": include only with delta "update" and an explicit
     what_changed naming the material delta.
   - "unseen": adjudicate the near matches yourself before treating as new.
   Record every drop as an omitted entry {story_key, urls, reason}.
4. Publish one briefing.publish call:
   - date, edition "morning", timezone "America/Los_Angeles", generated_at,
     summary_md (5-9 bullets), sections ordered by topic section_order,
     omitted, idempotency_key "briefing-<date>-morning".
   - Item style per topic instructions: headline_md is one bold linked
     sentence to the original publisher; body_md one to two sentences;
     why_it_matters always; detail_md carries the fuller brief (3-5
     sentences, measurements, context) for expand-in-place; story {key,
     urls, title, entities, event_at} on every news item — reuse story keys
     from dedupe results verbatim; times with published_at/event_at/
     first_seen_at when known.
   - Include "health" only when genuinely fresh early data exists.
5. Deliver the usual concise iMessage summary linking the briefing.
6. memory.checkpoint the run: objective, include/omit decisions, publish
   receipt refs.
```

## 10:00 health prompt

```text
Add the health update to today's briefing.

Call briefing.topics; follow the health topic instructions. Gather the
wearable read (fresh Ithrion context; sleep/recovery/readiness, HRV/RHR/
stress/body battery/training readiness, load context, one practical
non-diagnostic recommendation; Monday adds the prior-week look-back).

Re-publish via briefing.publish with the same date and edition "morning":
send the FULL edition payload — first read the current one with memory.read
on today's Briefings/<year>/Morning briefing - <date>.md metadata (or GET
the edition), keep every existing section and omitted entry unchanged, and
add/replace the health section. Set idempotency_key
"briefing-<date>-health-update". The delta in the response should show only
the health item(s); verify and report if not. Send the concise iMessage.
Fail closed on any Straylight error.
```

## Intraday alert checks (topic mode "immediate" successor)

Topics with instructions calling for intraday attention run the same loop at
their cadence: briefing.topics → research the one topic → briefing.dedupe →
if a material verified delta exists, re-publish the day's edition with the
new/updated item (delta "new"/"update", what_changed set) and deliver via
iMessage prefixed "Update:". Suppressions are recorded in omitted. Never
re-alert on corroboration, cosmetic updates, or aggregator rediscovery.

## Readiness checker

Update the stale "MCP readiness" check to probe current tool names:
memory.open, memory.query, memory.read, memory.write, memory.checkpoint,
memory.status, briefing.publish, briefing.dedupe, briefing.topics.

## Initial topic seeding

Before the first structured run, create the topic entries (SPA Topics page or
memory.write) migrating the standing configuration: ai, world-markets, games,
f1, stocks, joyeuse, charlemagne, discord-digest, x-digest, health, trackers,
ski (mode paused). Bodies port the preference prose from
`agent-memory/aether/memory/topics/briefings-news-stocks.md` and the current
cron prompt section rules.

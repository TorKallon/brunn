# Obsidian vault architecture plan

Created: 2026-04-18
Updated: 2026-04-18 13:24 PDT
Updated: 2026-04-18 13:18 PDT

## Related notes
- [[Projects/Metis/Metis|Metis]]
- [[Vault rules]]
- [[HOME]]
- [[Projects/Metis/Obsidian knowledge base master plan|Obsidian knowledge base master plan]]

## Current vault, in plain English
This vault is already useful, but it currently behaves more like an **active memo dropbox** than a true lifelong knowledge base.

### What exists now
- ~63 markdown notes, plus a few PDFs/CSVs
- Top-level folders: `Inbox/`, `Briefings/`, `Research/`, `References/`, `Strategy/`
- Most notes still live in the **root**, not in folders
- Strong recurring note types:
  - `Morning briefing - YYYY-MM-DD` daily operating briefs
  - Charlemagne cost / infra research packets
  - project analysis notes for Joyeuse, Ithrion, F1, ski, setup docs, etc.
- Obsidian core features are enabled for links, backlinks, daily notes, templates, bookmarks, graph, and properties

### What is already working well
- File naming is clear and human-readable
- Dated notes are consistent and easy to sort
- Notes are usually well-structured, with summaries and sections
- Research outputs are substantive, not shallow
- The vault already separates some work into `Research/`, `References/`, `Strategy/`, and `Inbox/`

## What kind of vault this is today
The dominant pattern is:
1. gather information
2. produce a briefing, memo, or analysis note
3. save it as a mostly standalone file

That is good for output capture.
It is weak for **accumulating reusable knowledge over years**.

### What is missing
- almost no internal linking
- no real index or home note
- no consistent distinction between **projects**, **ongoing life areas**, and **evergreen topics**
- root is overloaded and acting as a catch-all
- references and conclusions are mixed together in places
- no obvious archive boundary for inactive material
- no dedicated attachment/assets location
- very little permanent personal knowledge yet: people, places, health, finances, family systems, routines, decisions, lessons learned

## Recommendation: lightweight lifelong architecture
Do **not** rebuild this into a heavy PKM system.
The right move is a small amount of structure that supports both:
- fast capture and AI-generated outputs
- slow accumulation of durable personal knowledge

## Related links
- [[Projects/Metis/Metis|Metis]]
- [[Projects/Metis/Folder structure|Folder structure]]
- [[Projects/Metis/Navigation and retrieval model|Navigation and retrieval model]]
- [[Projects/Metis/Obsidian knowledge base master plan|Obsidian knowledge base master plan]]

## Target top-level structure
I would move toward this over time:

- `Inbox/` — raw captures, imports, temporary holding area
- `Journal/` — dated notes, briefings, logs, daily notes
- `Projects/` — active finite efforts with an outcome
- `Areas/` — ongoing parts of life with no end date
- `Topics/` — evergreen wiki-style notes for recurring subjects
- `References/` — source docs, supporting material, manuals, PDFs
- `Archive/` — completed or inactive material
- `Assets/` — attachments, images, PDFs if you want one common home

## How this maps to the current vault
### Keep
- `Inbox/`
- `References/`
- the memo-writing habit
- research-style notes

### Change gradually
- move all dated briefings into `Journal/`
- stop saving ordinary notes in the root
- treat `Research/` and `Strategy/` as temporary categories, then fold most of that work into either:
  - `Projects/` if it is tied to an active initiative
  - `Topics/` if it is evergreen
  - `Areas/` if it belongs to an ongoing life domain

## The most important architectural shift
Add a thin evergreen layer.

For any subject that recurs, create one stable note in `Topics/` or `Areas/` and let new dated notes point back to it.

Examples:
- `Topics/Charlemagne.md`
- `Topics/Joyeuse.md`
- `Topics/F1.md`
- `Areas/Health.md`
- `Areas/Finances.md`
- `Areas/Skiing.md`
- `Areas/Career.md`
- `Areas/Home systems.md`
- `Topics/AI workflows.md`

These should be short living notes, not polished essays.
They become the place where context accumulates.

## Minimal note types to use going forward
You only really need 5 note types:
- **capture** = inbox item
- **log** = dated journal/briefing note
- **project** = active effort
- **area** = ongoing responsibility/domain
- **topic** = evergreen knowledge page

That is enough.
No elaborate tagging system required.

## Simple rules that keep the vault healthy
1. **Nothing new goes in root** except maybe one home note.
2. **If a topic comes up twice, give it a `Topics/` or `Areas/` note.**
3. **Dated notes live in `Journal/`.**
4. **Source material lives in `References/` or `Assets/`, not mixed into summary notes.**
5. **Finished projects get moved to `Archive/`.**
6. **Use links lightly but consistently.** Every new project or dated note should link to the relevant ongoing page.

## What I would not do
- no heavy PARA bureaucracy
- no atomic-note rewrite
- no Zettelkasten purity project
- no mandatory frontmatter on everything
- no giant folder migration all at once

## Best practical starting moves
1. Create a single home note, like `Start here.md`
2. Create `Journal/`, `Projects/`, `Areas/`, `Topics/`, and `Archive/`
3. Move all morning briefings into `Journal/`
4. Create 5 to 8 high-value evergreen pages for recurring subjects
5. Use the root less and less until it becomes clean

## Bottom line
The vault already has strong raw material.
What it lacks is not content, but **durable structure**.

The best next version is:
- keep the current memo workflow
- add a small evergreen wiki layer
- separate dated logs from enduring knowledge
- make the root stop being the default destination

That would turn this from a smart output folder into a practical lifelong knowledge base without making it annoying to maintain.

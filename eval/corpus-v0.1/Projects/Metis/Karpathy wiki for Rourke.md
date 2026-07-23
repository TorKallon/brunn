# Karpathy wiki for Rourke

Created: 2026-04-18
Updated: 2026-04-18 13:18 PDT

## Quick read
Your vault already works well as a **working memo archive**. It is active, useful, and full of real outputs. What it does **not** yet have is a lightweight layer of evergreen pages that turns those outputs into a reusable personal wiki.

My recommendation is a **thin wiki on top of the current vault**, not a big re-org.

## Related notes
- [[Projects/Metis/Metis|Metis]]
- [[Research/Karpathy wiki pattern research|Karpathy wiki pattern research]]
- [[Vault rules]]
- [[HOME]]

## What the vault looks like now

### Current structure
- Mostly flat root with many active notes
- Folders in use: `Research/`, `Briefings/`, `References/`, `Strategy/`, `Inbox/`
- `.obsidian/` is configured with daily notes, templates, graph, bookmarks, sync, and Bases enabled
- About 61 markdown notes, plus a few PDFs, CSVs, and Obsidian config files

### Main workflows already visible
- **Daily briefings**: recurring `Morning briefing - YYYY-MM-DD` notes
- **Project research packets**: especially Charlemagne and Joyeuse
- **Strategy / writing support**: e.g. X voice strategy
- **Operational reference**: setup guides, insurance info, season guides
- **Drop-zone / source material**: `Inbox/` and `References/`

### What is missing today
- Very little cross-linking
- Almost no tags/frontmatter metadata
- Few canonical pages for recurring topics
- Root is doing too much work as a catch-all

This means the vault is currently better at **storing finished work** than at **building cumulative knowledge**.

## Best Karpathy-style adaptation for this vault
The best fit is:

1. **Keep dated memos and briefings as-is**
2. **Add evergreen topic pages for recurring subjects**
3. **Link every new dated note to one canonical page**
4. **Use the wiki pages as the place where context accumulates**

In other words: keep the memo-writing habit, but add a small set of stable pages that answer:
- What is this thing?
- Why does it matter?
- What is the current state?
- What are the key links / notes / decisions?

## What to keep
- The memo style, which is already strong
- `Research/` as a place for deeper outputs
- `References/` for durable source material
- `Inbox/` for raw inputs
- Daily briefings as a recurring operating rhythm

## What to change
- Stop letting the root become the default home for everything
- Create one canonical page per repeated topic
- Add wikilinks whenever a note belongs to an ongoing subject
- Distinguish clearly between:
  - **logs** (dated notes)
  - **wiki pages** (evergreen summaries)
  - **references** (raw/supporting material)

## Minimal architecture with the most value
I would start with just **one new top-level folder**:

- `Wiki/`

And keep the rest largely intact.

### Suggested shape
- `Inbox/` = raw captures, files, imports
- `Briefings/` = daily/periodic brief outputs
- `Research/` = deeper analyses and recommendations
- `References/` = source docs and durable factual notes
- `Strategy/` = higher-level personal/professional planning
- `Wiki/` = evergreen topic pages

## First wiki pages I would create
Based on this vault, the highest-value pages are probably:

- `Wiki/Charlemagne.md`
- `Wiki/Joyeuse.md`
- `Wiki/Nyx + Aether.md`
- `Wiki/Rourke.md`
- `Wiki/F1.md`
- `Wiki/Skiing.md`
- `Wiki/AI workflows.md`

Not all at once. Start with the subjects that recur most.

## Simple page template for wiki notes
Each wiki page should stay short and useful:

### Suggested sections
- **What it is**
- **Current state**
- **Key facts**
- **Open questions**
- **Recent notes**
- **Related references**

That is enough to make the page valuable without turning maintenance into a chore.

## Working rule going forward
A good minimal rule:

- If a topic comes up once, it can stay a normal note.
- If it comes up twice, give it a `Wiki/` page.
- If you write a dated memo, link it from the relevant wiki page.
- If a briefing contains durable insight, promote that insight onto the wiki page.

## My strongest recommendation
Do **not** rebuild the vault around atomic notes, heavy metadata, or an elaborate PARA/Zettelkasten scheme.

For this vault, the highest-value move is much simpler:

**preserve the current memo-and-briefing workflow, then add a thin evergreen wiki layer that makes recurring topics legible over time.**

That gets you most of the benefit with very little friction.

## Nice-to-have later, not now
- Move daily briefings into a tighter subfolder structure
- Add minimal frontmatter like `type`, `topic`, `date`
- Use Bases only after note types become more consistent
- Gradually reduce root clutter by moving new notes into homes by default

## Bottom line
Right now the vault is a strong **AI-assisted working notebook**.
With one small structural change, it could become a much better **personal operating wiki**.

The move I would make is: **add `Wiki/`, create canonical pages for recurring subjects, and let dated notes feed those pages instead of trying to replace them.**

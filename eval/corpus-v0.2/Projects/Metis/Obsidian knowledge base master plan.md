Updated: 2026-04-18 13:18 PDT

## Goal
Turn Obsidian into your primary long-term knowledge base, not just a sink for LLM output, and use the UpNote migration as the forcing function to organize it correctly.

## Related notes
- [[Projects/Metis/Metis|Metis]]
- [[Vault rules]]
- [[Projects/Metis/Obsidian vault architecture plan|Obsidian vault architecture plan]]
- [[Projects/Metis/UpNote to Obsidian migration plan|UpNote to Obsidian migration plan]]

## Core recommendation
Do this as a **staged migration + lightweight information architecture**, not a giant one-shot reorg and not a heavy PKM system.

The right model for you is:
- Obsidian becomes the **single home** for durable knowledge.
- Existing LLM output notes stay useful, but stop being the whole system.
- UpNote content is migrated into Obsidian through a **staging vault/process first**, not directly into your live vault.
- The vault gets a simple structure that supports both human capture and LLM-assisted synthesis.
- Over time, recurring subjects get rolled into evergreen wiki pages.

## What this means in practice
Your vault should support four kinds of things:
1. **Capture**: quick notes, inbox items, clipped thoughts
2. **Work**: project notes, research packets, strategy docs, briefings
3. **Reference**: durable facts, setup docs, operating notes, contacts, procedures
4. **Knowledge**: evergreen topic pages that synthesize what matters over time

That gives you a system that is simple enough to use daily but structured enough to compound.

## Recommended target structure
Use a lightweight top-level structure:

- `Inbox/`
- `Journal/`
- `Projects/`
- `Areas/`
- `Topics/`
- `References/`
- `Briefings/`
- `Archive/`
- `Assets/` (optional, for attachments)

### Meaning of each folder
- **Inbox**: temporary landing zone for uncategorized notes and captures
- **Journal**: dated daily notes, logs, trip notes, scratch chronology
- **Projects**: active finite efforts with an end state (Joyeuse, Charlemagne cost work, travel planning, etc.)
- **Areas**: ongoing parts of life/work without a clean end date (family logistics, finances, health, home systems, public voice)
- **Topics**: evergreen knowledge pages for recurring subjects (F1, skiing, AI workflows, AWS cost model, Crystal Mountain, etc.)
- **References**: stable factual/operational docs, setup notes, command references, account/process docs
- **Briefings**: generated daily/weekly briefing artifacts
- **Archive**: inactive material that should still be preserved
- **Assets**: images, PDFs, exported files, attachments

## What not to do
Do **not**:
- convert everything into atomic Zettelkasten notes
- build a metadata-heavy system first
- spend days arguing about PARA vs Johnny Decimal vs tags vs folders
- manually reorganize everything before migration
- make the live vault the first place you experiment with conversion

That is the trap.

## The right role for the “Karpathy wiki” idea here
For you, the value is not “LLM controls the vault.”
The value is:
- raw notes and exports go in
- recurring subjects get distilled into stable pages
- the stable pages become the real working memory layer

So the right adaptation is a **thin wiki layer**, not a fully autonomous self-editing wiki.

Practically, that means creating evergreen pages in `Topics/` and `Areas/` for subjects that recur.

Examples:
- `Topics/Charlemagne.md`
- `Topics/Joyeuse.md`
- `Topics/AI workflows.md`
- `Topics/F1.md`
- `Topics/Skiing.md`
- `Areas/Family logistics.md`
- `Areas/Public voice project.md`
- `References/Nyx and local systems.md`

Rule of thumb: if a subject comes up more than twice, it probably deserves a stable page.

## Holistic migration plan

### Phase 0: Define the system before moving content
Before importing UpNote, set the destination model:
- create the target top-level folders in Obsidian
- decide attachment handling (`Assets/` or alongside notes)
- decide naming rules
- decide what counts as Project vs Area vs Topic vs Reference
- create a short “vault rules” note so future-you and future-me stay consistent

### Phase 1: Protect the source of truth
Before touching anything:
- create a full native **UpNote backup**
- keep that backup untouched as the recovery source
- export both:
  - **HTML** (better formatting fidelity)
  - **Markdown** (better text/search portability)

Why both:
- Markdown alone is often weaker on tables/code/rich formatting
- HTML alone is often weaker on links/structure/search ergonomics
- the dual-export approach gives you the best chance of reconstructing a good Obsidian result

### Phase 2: Use a staging vault, not your live vault
Create a temporary migration workspace or staging vault.
Do the first import there.

Best current primary path:
- use **UpNote_To_Obsidian** as the main converter path for fidelity

Likely fallback / alternate path:
- use **Jimmy** if internal links and notebook hierarchy matter more than perfect formatting on some note types

The reason to stage first is simple: you want to measure damage before polluting your real system.

### Phase 3: Sample validation before full import
Do not migrate everything first.
Pick a representative sample, ideally:
- 5 simple text notes
- 5 notes with tables
- 5 notes with images/files
- 5 notes with internal links
- 3-5 notes with code blocks / rich formatting
- 3 notebook/folder examples with hierarchy

Validate:
- table fidelity
- attachment placement
- link preservation
- filename cleanliness
- folder mapping
- searchability in Obsidian
- readability on mobile and desktop

If the sample is bad, adjust before full import.

### Phase 4: Full import into staging
Once the sample is acceptable:
- import all UpNote content into staging
- keep imported material separated from hand-authored Obsidian notes initially
- normalize filenames and folder structure only after import succeeds
- do not try to manually “perfect” every note

### Phase 5: Triage and reshape
After import, classify notes into buckets:
- **Keep as-is**: most historical notes
- **Promote to evergreen page**: recurring topics and durable summaries
- **Archive**: old or low-value material
- **Split/repair**: only high-value notes with broken tables or formatting

This is where the knowledge base actually forms.

## How to handle the table problem
Tables were one of the blockers last time, so be explicit:

- Expect **Markdown export alone** to be insufficient for some UpNote tables.
- Treat **HTML export as the formatting source of truth** for table-heavy notes.
- For notes where tables are mission-critical, preserve the original exported artifact if needed.
- Accept that some complex tables may need one of these treatments:
  - clean Markdown table rewrite
  - attachment/PDF preservation
  - embed/screenshot fallback for ugly but important historical content

Not every historical table deserves perfect reconstruction.
Reserve manual repair for high-value notes.

## How to keep this from becoming a mess again
You need a few operating rules.

### New-note rules
- New notes should almost never go in the root.
- Default destinations:
  - quick capture -> `Inbox/`
  - dated scratch/log -> `Journal/`
  - active work -> `Projects/`
  - stable knowledge -> `Topics/`, `Areas/`, or `References/`

### Evergreen rules
- If a subject repeats, create a canonical page.
- Link project notes and daily notes back to the canonical page.
- Canonical pages should summarize, not duplicate every source note.

### Archive rules
- Old project notes move to `Archive/Projects/...`
- Old imports that are mostly historical can stay archived and searchable without being front-and-center

## Best role for me in this system
I should help in four ways:
1. **Triage** imported notes into the right buckets
2. **Repair** high-value formatting issues selectively
3. **Roll up** recurring knowledge into evergreen pages
4. **Maintain** a few canonical pages over time as new work happens

This is the right level of LLM involvement. Useful, compounding, not overly magical.

## Suggested first canonical pages
After the system is created, I would start with these:
- `Topics/Charlemagne.md`
- `Topics/Joyeuse.md`
- `Topics/AI workflows.md`
- `Topics/F1.md`
- `Topics/Skiing.md`
- `Areas/Family logistics.md`
- `Areas/Public voice project.md`
- `References/Nyx and local systems.md`
- `References/Travel preferences.md`
- `References/Tooling and automation.md`

## Recommended execution order
1. Create target folder structure
2. Create a short vault-rules note
3. Export UpNote backup + HTML + Markdown
4. Build staging import workflow
5. Validate sample notes
6. Run full staging import
7. Triage imported content
8. Move accepted content into the live vault structure
9. Create first evergreen pages
10. Gradually backfill links and summaries during normal use

## My bottom line
Yes, I think this is the right time to make Obsidian your real knowledge base.
But the winning move is not “import everything and hope.”
It is:
- design a simple structure
- migrate safely through staging
- preserve the source
- accept imperfect historical fidelity where necessary
- build an evergreen knowledge layer on top of the imported archive and new notes

That gives you one system for everything, without turning the whole effort into a months-long PKM project.

Created: 2026-04-18 11:27 PDT
Updated: 2026-05-03 16:42 PDT
Status: Active

## Purpose
Metis is the project to turn Obsidian into Rourke's primary long-term knowledge base and migrate durable knowledge into it, including material currently living in UpNote.

## Outcome
A clean, usable Obsidian system that:
- becomes the default home for durable personal and project knowledge
- has a lightweight but durable folder structure
- supports both human capture and LLM-assisted synthesis
- gives coding agents and harnesses a compact routing map through the vault-root [[INDEX|shared knowledge index]]
- safely imports and stages UpNote content before full adoption
- accumulates evergreen wiki pages over time instead of staying just an LLM output archive

## Current framing
This is not just a migration project.
It is a knowledge-base design project with a migration component.

The current working model uses the root [[INDEX|INDEX.md]] note as the canonical shared routing map for coding agents and harnesses. Repo-level instruction files for Codex and other agents should point to that vault index instead of duplicating the project map, then verify implementation details against the repo docs, tests, and source code.

## Success criteria
- Obsidian has a clear top-level structure
- new notes no longer default to root
- UpNote content is backed up and imported through staging safely
- high-value historical notes remain searchable and readable
- recurring subjects are promoted into evergreen notes
- the vault becomes useful as an everyday operating system, not just storage
- external harnesses can find project context through [[INDEX]] without loading the whole vault

## Active docs
- [[INDEX|Shared knowledge index]]
- [[Research/Cross-harness knowledge base plan - 2026-04-25|Cross-harness knowledge base plan]]
- [[Projects/Metis/Codex vault write access and AGENTS setup|Codex vault write access and AGENTS setup]]
- [[Projects/Metis/Plan|Plan]]
- [[Projects/Metis/Worklog|Worklog]]
- [[Projects/Metis/Decisions|Decisions]]
- [[Projects/Metis/Open questions|Open questions]]
- [[Projects/Metis/Document processing pipeline|Document processing pipeline]]
- [[Projects/Metis/Use cases|Use cases]]
- [[Projects/Metis/Folder structure|Folder structure]]
- [[Projects/Metis/Document note template|Document note template]]
- [[Projects/Metis/Finance summary template|Finance summary template]]
- [[Projects/Metis/Health history template|Health history template]]
- [[Projects/Metis/Trip workspace template|Trip workspace template]]
- [[Projects/Metis/Navigation and retrieval model|Navigation and retrieval model]]
- [[Projects/Metis/Vault semantic search|Vault semantic search]]
- [[Projects/Metis/Project status model|Project status model]]
- [[Projects/Metis/UpNote migration runbook|UpNote migration runbook]]
- [[Projects/Metis/Scanned documents in Drive runbook|Scanned documents in Drive runbook]]
- [[Projects/Metis/Rune personal context bridge plan - 2026-04-21|Rune personal context bridge plan]]
- [[Projects/Metis/Rune bridge API handoff - 2026-04-21|Rune bridge API handoff]]
- [[Records/Imports/Import cleanup dashboard|Import cleanup dashboard]]
- [[Projects/Metis/Karpathy wiki for Rourke|Karpathy wiki for Rourke]]
- [[Research/Karpathy wiki pattern research|Karpathy wiki pattern research]]
- [[Projects/Metis/Obsidian knowledge base master plan|Obsidian knowledge base master plan]]
- [[Projects/Metis/Obsidian vault architecture plan|Obsidian vault architecture plan]]
- [[Projects/Metis/UpNote to Obsidian migration plan|UpNote to Obsidian migration plan]]

## Next actions
- keep [[INDEX]] compact and current as project/repo routing changes
- keep Metis repo instructions for Codex and other agents pointed at the vault-root index
- continue lightweight vault maintenance and weekly structure sweeps
- keep improving scanned-document and UpNote migration workflows as real cases surface
- promote recurring imported material into evergreen wiki pages when the pattern is real

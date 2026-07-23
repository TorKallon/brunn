Created: 2026-04-18 11:30 PDT
Updated: 2026-04-18 11:30 PDT

## Goal
Bring the useful content of scanned historical documents into Obsidian as reliable, searchable knowledge, while keeping Google Drive as the source of truth for the original scanned files.

## Recommended model
Do **not** import the whole pile of scans into Obsidian as raw binaries first.

Instead:
- keep originals in Google Drive
- create Obsidian notes that act as **document records**
- store or generate **raw OCR text** and a **cleaned summary/structured extraction**
- link each note back to the Google Drive original

This gives you:
- searchability in Obsidian
- durable summaries and extracted facts
- lower vault bloat
- a clean separation between source artifact and knowledge object

## Best architecture
### Source of truth
- Google Drive remains the home of original PDFs/scans
- each document record should include a stable Google Drive link

### Obsidian layer
For each important document, create either:
1. a **document note** for the individual file, or
2. a **document bundle note** for a coherent group of related records

Recommended default: one note per important document.

### Suggested location
- `Archive/Documents/` for imported historical document records
- optional `Assets/Documents OCR/` only if we decide to keep raw OCR text as separate sidecar files instead of embedding it in the note

## Recommended note shape
Each document note should contain:
- title
- approximate document date
- document type
- people/orgs involved
- source/origin
- Google Drive link
- confidence / OCR quality note
- short summary
- extracted key facts
- optional raw OCR block or linked sidecar text
- links to relevant project/topic/area notes

## Important design principle
Use **two layers of text** when possible:
1. **raw OCR text** (preserve what the scanner/OCR actually saw)
2. **cleaned / interpreted layer** (human- or LLM-assisted summary and extracted facts)

Do not overwrite raw OCR with cleaned prose.
That is how you preserve trust.

## Why this is the right fit
This setup is much better than either extreme:

### Better than only Google Drive
- richer synthesis
- links into projects, people, areas, and topics
- easier cumulative knowledge building

### Better than dumping all PDFs into Obsidian
- avoids vault bloat
- avoids attachment chaos
- avoids treating poor OCR as final truth
- keeps source artifacts in the system already good at storing them

## Best workflow
### Phase 1: pilot
Pick a small representative sample:
- personal records
- financial/tax records
- medical or insurance docs
- old contracts / official letters
- anything table-heavy or messy

For each sample document:
- confirm Google Drive link strategy
- capture OCR text
- assess OCR quality
- create a document record note
- extract summary + key facts
- link to relevant evergreen notes

### Phase 2: define rules
Decide:
- which documents deserve full notes vs just archive references
- what metadata fields matter
- whether raw OCR lives inside the note or in sidecar text files
- naming conventions

### Phase 3: scale
Only after the pilot feels good:
- process batches
- prioritize high-value categories first
- let low-value documents stay lightly indexed rather than fully normalized

## Triage model
Not every scan deserves the same treatment.

### Tier A: high-value, reusable
Create a full document record note.
Examples:
- property / home docs
- tax / finance anchor docs
- contracts
- identity / legal / insurance docs
- major family records

### Tier B: useful but mostly archival
Create a short note or batch note with summary + Drive links.

### Tier C: low-value archive
Leave primarily in Drive, maybe only referenced in an index note.

## Risks and mitigations
### OCR quality
Mitigation:
- preserve raw OCR
- mark low-confidence docs
- manually verify high-value extracts

### Table fidelity
Mitigation:
- treat tables as structured extraction work, not just plain OCR output
- for crucial documents, keep table screenshots/snippets or manually reconstructed summaries

### Over-processing everything
Mitigation:
- start with high-value categories only
- do not normalize the whole archive before proving the workflow

## My current recommendation
Yes, add historical scanned documents to Metis as a first-class workstream.
But do it as:
- **Drive for originals**
- **Obsidian for document records, OCR text, summaries, and links**
- **pilot first, then scale**

That gives you the knowledge value without making the vault a document graveyard.

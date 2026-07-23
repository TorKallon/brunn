Created: 2026-04-18 11:27 PDT
Updated: 2026-04-18 11:42 PDT

## Objective
Build a holistic plan to make Obsidian the primary knowledge base and migrate UpNote content into it safely.

## Workstreams
### 1. Information architecture
- define top-level folders
- define note destination rules
- define archive rules
- define evergreen knowledge layer
- make the structure serve the real operating modes: retrieval, planning, coordination, project work, and research

### 2. Migration design
- preserve native UpNote backup
- export HTML and Markdown
- validate converter path in staging
- identify failure modes for tables, links, and attachments

### 3. Vault operating model
- define how new notes get created
- define when a subject becomes a stable topic page
- define what stays as dated artifact vs evergreen note
- define the role of LLM-maintained summaries and rollups

### 4. Historical documents layer
- define the role of scanned historical documents in the knowledge base
- keep Google Drive as source of truth for originals
- design document-record notes in Obsidian
- preserve raw OCR separately from cleaned summaries/extractions
- define the extraction pipeline and confidence rules
- pilot a small batch before scaling

### 5. Execution
- create live folder structure
- create vault rules note
- stand up staging import flow
- run sample import
- review sample quality
- run full migration when approved

## Draft phases
### Phase 0
Finalize architecture and rules.

### Phase 1
Create backup and exports from UpNote.

### Phase 2
Build staging migration workflow.

### Phase 3
Run representative sample validation.

### Phase 4
Import full archive into staging.

### Phase 5
Triage, repair high-value content, and promote evergreen notes.

### Phase 6
Pilot the historical-document workflow on a representative batch.

### Phase 7
Adopt Obsidian as primary system.

## Immediate next step
Turn the master plan into an executable checklist, approve the target folder structure against the real use cases, and design the historical-document pilot.

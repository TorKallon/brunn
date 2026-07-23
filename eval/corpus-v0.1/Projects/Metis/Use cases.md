Created: 2026-04-18 11:42 PDT
Updated: 2026-04-18 11:55 PDT

## Why Metis exists
These are the real jobs the vault needs to do.
Not just note storage, but reliable retrieval, planning, coordination, project support, and research.

## Core use cases
### 1. Random high-value retrieval
Examples:
- what is my son's social security number?
- what was my gross on 2025's taxes?
- find me the last bill from Comcast

Implications:
- the vault must support highly reliable retrieval of sensitive personal records
- scanned documents need strong metadata and links back to source originals
- account/document records need consistent naming and document types
- sensitive fields should be easy to retrieve when asked in a direct trusted context
- source links and raw OCR should remain available for verification
- retrieval should work primarily through assistant queries, search, and home-page navigation, not manual browsing

### 2. Joint vacation planning with Jen, plus weekend trips
Examples:
- planning a trip together
- keeping itinerary, reservations, ideas, and documents in one place
- getting a daily update while traveling
- lighter-weight weekend trip planning

Implications:
- Metis should support a trip workspace with both planning and active-trip operations
- notes should be able to roll up reservations, places, logistics, and day plans
- daily travel briefings should be generatable from the underlying trip notes plus calendar items
- the same structure should scale down to lighter weekend trips, not just full vacations
- family trip notes should be easy to update collaboratively, even if the final assistant access pattern is mostly through chat

### 3. Ski season travel planning and coordination
Examples:
- race travel
- ski weekends
- lodging / travel / schedule coordination
- pulling in Google Calendar and Google Sheets data
- household-adjacent logistics that often overlap with family travel planning

Implications:
- Metis needs a place for seasonal plans and event-specific trip notes
- Google integration matters here more than abstract PKM structure does
- calendar events, race schedules, and sheets-based planning should be linkable into notes or summarized into them
- this is both an archive problem and an operational dashboard problem

### 4. Project tracking and working memory
Examples:
- Joyeuse
- Ithrion
- Charlemagne

Implications:
- project notes need to function as working memory, not just archives
- each project should have a stable home with plans, open questions, decisions, references, and links to code/repos
- the vault should accumulate durable context that reduces re-explaining the same project over time
- evergreen project pages should sit above raw research/output notes
- project overview notes should expose lightweight status so Aether can answer project portfolio questions quickly

### 5. Financial planning and status
Examples:
- financial planning
- current status and trend tracking
- household money questions
- historical retrieval from bills, taxes, statements, and planning notes

Implications:
- Metis should support both archival financial records and ongoing planning views
- this needs summary pages as well as document-level source records
- financial notes should be able to roll up account snapshots, taxes, recurring bills, and planning assumptions
- historical source docs should stay linked so summaries remain auditable

### 6. Health history and status
Examples:
- MRI results
- blood tests
- medical history
- longitudinal health tracking

Implications:
- Metis should support a durable medical-history layer, not just isolated files
- test results and reports should be easy to retrieve by date, provider, and topic
- recurring metrics and major findings should be promotable into longitudinal summary notes
- this is highly sensitive and should be treated as one of the most privacy-critical parts of the vault

### 7. Random research and thought experiments
Examples:
- exploratory research
- technical investigations
- personal thought experiments
- questions that may or may not become enduring topics

Implications:
- capture needs to be easy and low-friction
- not all research deserves deep structure at first
- if a topic recurs, it should graduate into a stable topic page
- the system should tolerate messy exploration without forcing premature organization

## Architectural consequences
These use cases imply that Metis needs to support seven distinct modes well:
- **records retrieval**
- **operational planning**
- **calendar/sheet-backed coordination**
- **project working memory**
- **financial planning and status**
- **health history and status**
- **exploratory research**

That pushes the design toward a hybrid system:
- document records for sensitive archival retrieval
- project and area notes for ongoing life/work coordination
- dedicated financial and health summary layers above raw records
- evergreen topic pages for recurring knowledge
- lightweight inbox/research capture for messy thinking
- integrations where they materially improve daily usefulness

## Design priorities sharpened by these use cases
### 1. Retrieval trust matters more than elegance
If the vault cannot reliably answer a high-stakes retrieval question, it fails one of its most valuable jobs.

### 1a. Navigation should be assistant/search/home-first
The design should assume Rourke will not manually sort, browse, or maintain categories by hand.

### 2. Operational usefulness matters more than taxonomy purity
Trip planning, ski coordination, and active projects matter more than having a beautiful PKM philosophy.

### 3. Sensitive-record handling is first-class
The design must assume some notes and extracted fields are highly sensitive.

### 4. Health and finance need longitudinal views
These are not just filing-cabinet categories. They need both raw records and durable rollup pages that make history understandable over time.

### 5. Integrations should be pragmatic
Calendar and Sheets integration should be added where they clearly improve coordination workflows.

### 6. Evergreen pages should emerge from use
Do not over-design the topic layer before real use reveals what deserves permanence.

## What this changes
Metis is not just:
- an Obsidian cleanup project
- an UpNote migration project
- a scanned-document ingestion project

It is a broader personal knowledge-and-operations system.

## Recommended next design tasks
- define the folder structure against these seven use-case modes
- define the document note template for sensitive records
- define trip/project note templates
- define financial and health summary-note patterns
- define the vault rules note
- define where calendar/sheets-derived notes or summaries should live
- define what information should be extracted into evergreen pages versus kept in operational notes

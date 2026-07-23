Created: 2026-04-18 12:01 PDT
Updated: 2026-04-26 16:44 PDT

## Goal
Define how Rourke should actually find and use information in the vault.

## Core rule
The vault should **not** assume Rourke will browse folders, manually sort notes, or do routine housekeeping.

The primary retrieval methods should be:
- asking Aether
- search
- starting from a root home page and following wiki-style links
- starting coding agents/harnesses from the root [[INDEX|shared knowledge index]]
- optionally bookmarking a few high-value notes

Folders still matter, but mainly as backend structure for consistency, not as the main user interface.

## Retrieval model
### 1. Assistant-first retrieval
The most important interface is asking questions in natural language.
Examples:
- what are all the projects I’m working on?
- what was my gross on 2025 taxes?
- find the last Comcast bill
- what did the MRI show?

Implication:
- notes need enough structure and linking that Aether can reliably answer from them
- sensitive records need clear provenance and source links

### 2. Search-first retrieval
If Rourke searches directly, search should work well without careful filing.

Implication:
- note titles should be predictable
- important documents need document type, issuer/provider, and date fields
- summaries should contain plain-language terms people actually search for

### 3. Home-page navigation
There should be a root note that acts as the main entry point into the vault.

This note should link to:
- active projects
- current trips
- finance
- health
- household/home
- key records indexes
- recent briefings
- important topic pages

The home page should function like a personal wiki front page, not a directory listing.

### 4. Agent/harness routing
Coding agents and less sophisticated harnesses need a predictable compact front door.

Implication:
- keep [[INDEX]] at the vault root as the canonical routing map
- keep it short and source-oriented, not narrative
- point repo-level `AGENTS.md` and `CLAUDE.md` files to it
- after using it to find context, verify implementation details against repo docs, tests, and source code

### 5. Small bookmark layer
A few especially important notes can be bookmarked or pinned.
Examples:
- Home
- Active projects
- Finance summary
- Health summary
- current trip

## Design consequences
### Folders are not the UX
Folder hierarchy should support consistency and automation, but should not be required for day-to-day navigation.

### Root-level notes matter
The root should contain only protected high-value jumping-off notes, currently:
- `Home.md`
- `INDEX.md`
- `Active projects.md`
- `Vault rules.md`
- `SCRATCH.md` if present

### Links matter more than location
A useful network of links between home, projects, areas, trips, records, and topics matters more than perfect categorization.

### Notes need better titles and front matter-like fields
Because search is a primary UX, notes should be named and structured for retrieval, not just for filing.

## Recommendation
Design Metis so that the main user journey is:
1. ask Aether
2. search
3. jump in from `Home`
4. route agents/harnesses through `INDEX.md`
5. occasionally use bookmarked notes

Anything requiring regular manual folder browsing should be treated as a design failure.

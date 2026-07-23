Created: 2026-04-18 12:01 PDT
Updated: 2026-04-18 12:01 PDT

## Goal
Support questions like: "what are all the projects I’m working on and what is the status of each?"

## Requirement
Metis needs a project status layer, not just scattered project notes.

## Recommended model
Each active project should have a stable overview note containing a small standard status block.

Suggested fields:
- Status: active | paused | blocked | completed | incubating
- Owner:
- Last updated:
- Current focus:
- Next step:
- Health: on track | at risk | stalled
- Links to plan/worklog/decisions

## Retrieval pattern
To answer project-status questions well, there should be an index note that links to all active projects.

Recommended note:
- `Active projects.md`

This note can include, for each project:
- project name
- one-line purpose
- current status
- current focus
- next step

## Why this matters
Without a shared status block, the answer to "what projects am I working on?" becomes vague and expensive to reconstruct every time.

With a shared pattern, Aether can answer quickly and the vault can also support manual review from one page.

## Suggested project overview shape
Each project's main page should include:
- Purpose
- Status
- Current focus
- Next actions
- Key links

## Recommendation
Eventually create:
- root or near-root `Active projects.md`
- per-project overview/status blocks
- lightweight project review habit, updated when a project meaningfully changes

This does not need to become formal project management.
It just needs enough structure that project state is legible.

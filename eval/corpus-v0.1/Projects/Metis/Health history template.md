Created: 2026-04-18 11:58 PDT
Updated: 2026-04-18 11:58 PDT

## Purpose
Template for a longitudinal health summary page in `Areas/Health/`.

## Template
```markdown
Created: {{date}}
Updated: {{date}}
Person: 
Scope: overall health | condition | test history | provider history
Related records: 

## Current summary

## Major history / timeline
- 

## Conditions / recurring concerns
- 

## Tests and imaging
- Date:
  - Test:
  - Provider:
  - Main result:
  - Linked source:

## Medications / treatment context
- 

## Providers
- 

## Trends / open questions
- 

## Next actions or follow-ups
- 
```

## Notes
- This is the keeper-of-history layer above raw medical documents.
- Raw reports, labs, and scans should live in `Records/Health/` and link back here.
- Avoid collapsing nuanced results into overconfident summaries.
- Preserve dates, providers, and source links carefully.

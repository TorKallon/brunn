Created: 2026-07-10 21:45 PDT
Updated: 2026-07-10 21:45 PDT
Status: Running

Related: [[Straylight]], [[Retrieval API - Initial Design]], [[Projects/Straylight/Open questions|Open questions]]

## Objective
Test the first Straylight product claim before selecting a storage engine: whether a stateful reasoning workspace recovers complete, source-bearing evidence more reliably than one-shot top-k retrieval while remaining competitive with direct filesystem access.

## Frozen corpus
- all top-level Markdown notes in `Projects/Metis`
- all top-level Markdown notes in `Projects/N24 RaceWatch`
- all top-level Markdown notes in `Projects/Home Network Improvements`
- all top-level Markdown notes in `Projects/Straylight`

Private, health, finance, family, and work-record folders are excluded.

## Frozen cases
The machine-readable manifest at `/Users/Shared/projects/straylight/eval/cases.json` contains 20 questions and 53 required evidence items. It was validated against the corpus before the retrieval implementation ran.

Manifest SHA-256: `050fa6041457bb63f43b0f1d67f549a2b180f7b1135ea2897d2f6c743425bf9e`

The cases cover exact facts, policy, temporal state, contradictions, quantitative evidence, API contracts, multi-source continuation, architecture, and authority boundaries.

## Compared surfaces
### Direct filesystem
Use lexical retrieval to identify likely files, then read up to three complete files under a 45,000-character budget.

### One-shot top-k
Return at most six Markdown-aware chunks under a 6,000-character budget. No follow-up retrieval is allowed.

### Minimal Memory Workspace
1. Open a compact corpus map.
2. Resolve the likely project from retrieval evidence.
3. Materialize the complete project when it is at most 45,000 characters.
4. Otherwise query initial chunks, read full sections and neighbors, follow relevant wiki links, and run a second pseudo-relevance-feedback query.
5. Keep progressive retrieval under 12,000 characters, excluding complete-project materialization.

## Scoring
- **Case pass:** every frozen evidence item required to answer the question is present.
- **Evidence recall:** recovered gold evidence items divided by all gold evidence items.
- **Retrieval cost:** returned characters and an approximate character-to-token conversion.
- **Failure set:** cases and specific evidence missed by each method.

## Interpretation boundary
This first run measures evidence availability and answerability. It does not sample a separate language model or grade generated prose. It is an engineering regression test, not an independent scientific evaluation.

## Decision gate
- A workspace recall improvement over one-shot retrieval supports advancing to a blinded model-answer evaluation.
- Material regression against direct filesystem access means the workspace cannot yet replace direct source tools.
- No storage-engine decision should be made from this retrieval-only run.


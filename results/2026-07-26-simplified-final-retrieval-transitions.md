Created: 2026-07-26T21:15:55-07:00
Updated: 2026-07-26T21:15:55-07:00
Status: Complete

Related: [[Straylight]], [[Projects/Straylight/Agent work evaluation results - 2026-07-10|Agent work evaluation results]]

# Straylight checkpoint transition evaluation

## Scope
- Model: `gpt-5.6-sol`
- Embeddings: `openai` / `text-embedding-3-small`
- Cards: 5 across Warmind, Charlemagne, Star Rupture, Switzerland, and Straylight
- Each card reopens a revision-N checkpoint with a fresh agent, introduces a revision-N+1 delta, and requires a source-preserving child checkpoint.

## Results
| Condition | Cases | Claims | Mean cumulative input | Mean cached input | Mean uncached input | Mean shell calls | Mean workspace calls |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Filesystem rebuild | 0/5 | 12/20 | 106,402 | 84,838 | 21,563 | 3.6 | n/a |
| Native Straylight checkpoint resume | 0/5 | 10/20 | 100,394 | 79,718 | 20,675 | 2.8 | 2.8 |

## Per-card results
| Card | Filesystem rebuild | Native Straylight checkpoint resume |
| --- | ---: | ---: |
| `warmind-alpha-budget-transition` | FAIL 3/4 | FAIL 2/4 |
| `charlemagne-storage-observation-transition` | FAIL 3/4 | FAIL 3/4 |
| `star-rupture-second-facturer-transition` | FAIL 1/4 | FAIL 0/4 |
| `switzerland-reservation-transition` | FAIL 3/4 | FAIL 3/4 |
| `straylight-api-gate-transition` | FAIL 2/4 | FAIL 2/4 |

## Findings
- Native Straylight checkpoint resume returned 107,777 model-visible service characters per card: 97,361 evidence text and 10,416 transport metadata. Replay-weighted output was 299,058 characters.
- Filesystem reconstruction recovered 12/20 claims; Native Straylight checkpoint resume recovered 10/20.
- Checkpoint resume reduced mean cumulative input by 6%, uncached input by 4%, and shell calls by 22% versus rebuilding from the filesystem.
- Native Straylight checkpoint resume committed 5/5 immutable child checkpoints with exact parent, pinned input revision, prior-source, and delta-source lineage.
- Four cards completed with `open -> checkpoint`; one used `open -> read -> checkpoint`. Mean service calls were 2.8, below the four-call gate.
- Mean elapsed time was 61.3s for checkpoint resume versus 57.4s for filesystem reconstruction.
- The OpenAI embedding index was built and available, but agents issued 2 semantic/lexical query calls in this suite because the parent checkpoint and explicit delta were sufficient. Semantic hit-rate improvement remains a separate evaluation gate.

## Gate
- Checkpoint-resume call budget: no more than 4 service calls per card.
- Resume pass additionally requires a session pinned to revision N+1, an immutable child checkpoint with exact parent linkage, and both prior and delta source references.
- Native resume additionally requires the committed child checkpoint to be read back through the HTTP session/checkpoint surface.
- Synthetic deltas are isolated evaluation fixtures and are not project, production, game, or booking truth.
- A pre-fix run that mounted workspace databases outside the Codex-writable case directory is retained with the result artifacts. Its zero-workspace score is an invalid harness failure, not a product result.

## Reproduce
```bash
cd /Users/Shared/projects/straylight
python3 transition_eval.py validate
STRAYLIGHT_API_URL=http://127.0.0.1:18110 STRAYLIGHT_EVAL_TOKEN='<owner read/write token>' python3 transition_eval.py run --filesystem-native --concurrency 2 --timeout 420 --out results/native-transitions.json
```

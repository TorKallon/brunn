Created: 2026-07-27T03:04:07-07:00
Updated: 2026-07-27T03:04:07-07:00
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
| Native Straylight checkpoint resume | 0/5 | 11/20 | 92,432 | 61,184 | 31,248 | 2.6 | 2.6 |

## Per-card results
| Card | Native Straylight checkpoint resume |
| --- | ---: |
| `warmind-alpha-budget-transition` | FAIL 3/4 |
| `charlemagne-storage-observation-transition` | FAIL 2/4 |
| `star-rupture-second-facturer-transition` | FAIL 2/4 |
| `switzerland-reservation-transition` | FAIL 3/4 |
| `straylight-api-gate-transition` | FAIL 1/4 |

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

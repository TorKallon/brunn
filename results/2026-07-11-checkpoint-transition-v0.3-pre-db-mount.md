Created: 2026-07-11T01:06:46-07:00
Updated: 2026-07-11T01:06:46-07:00
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
| Filesystem rebuild | 1/5 | 16/20 | 111,300 | 89,139 | 22,160 | 4.2 | n/a |
| Straylight checkpoint resume | 0/5 | 0/20 | 66,538 | 52,070 | 14,468 | 3.4 | 0.0 |

## Per-card results
| Card | Filesystem rebuild | Straylight checkpoint resume |
| --- | ---: | ---: |
| `warmind-alpha-budget-transition` | FAIL 3/4 | FAIL 0/4 |
| `charlemagne-storage-observation-transition` | FAIL 3/4 | FAIL 0/4 |
| `star-rupture-second-facturer-transition` | FAIL 3/4 | FAIL 0/4 |
| `switzerland-reservation-transition` | FAIL 3/4 | FAIL 0/4 |
| `straylight-api-gate-transition` | PASS 4/4 | FAIL 0/4 |

## Gate
- Workspace call budget: no more than 4 service calls per card.
- Workspace pass additionally requires an immutable child checkpoint at revision N+1, exact parent linkage, and both prior and delta source references.
- Synthetic deltas are isolated evaluation fixtures and are not project, production, game, or booking truth.

## Reproduce
```bash
cd /Users/Shared/projects/straylight
python3 transition_eval.py validate
python3 transition_eval.py run --embeddings openai --concurrency 2 --timeout 420 --out results/checkpoint-transition-v0.3.json --report '/Users/aether/obsidian/notes/Projects/Straylight/Checkpoint transition evaluation results - 2026-07-11.md'
```

Created: 2026-07-28T03:36:10-07:00
Updated: 2026-07-28T03:36:10-07:00
Status: Complete

Related: [[Straylight]], [[Projects/Straylight/Agent work evaluation results - 2026-07-10|Agent work evaluation results]]

# Straylight checkpoint transition evaluation

## Scope
- Model: `gpt-5.6-sol`
- Embeddings: `none` / `text-embedding-3-small`
- Cards: 5 across Warmind, Charlemagne, Star Rupture, Switzerland, and Straylight
- Each card reopens a revision-N checkpoint with a fresh agent, introduces a revision-N+1 delta, and requires a source-preserving child checkpoint.
- Service build revision: `232e7c6c8b8675d3ceeee3ff02a600ffdb95d2da`
- Expected runtime features: `{"intention_ledger":false,"lexical_single_scan":false,"read_path_roundtrip_v1":false,"resume_deltas":false,"search_char_cap":false,"search_fair_share":false,"search_section_demotion_top_n":null,"search_top1_hydration":false,"semantic_lane":false,"supersession_demotion":false,"verbatim_spans":false}`
- Actual runtime features: `{"allow_degraded_embeddings":true,"embed_cache":true,"embedding_backfill_batch_chunks":64,"embedding_backfill_foreground_status_timeout_ms":1000,"embedding_backfill_foreground_status_url_configured":true,"embedding_backfill_guard":true,"embedding_backfill_inter_batch_ms":250,"embedding_backfill_open_p95_limit_ms":120.0,"embedding_backfill_search_p95_limit_ms":107.0,"intention_ledger":false,"lexical_single_scan":false,"materialize_token_budget":24000,"observability_timings_ms":true,"read_path_roundtrip_v1":false,"resume_deltas":false,"search_char_cap":false,"search_fair_share":false,"search_section_demotion_top_n":null,"search_top1_hydration":false,"semantic_deadline_ms":300,"semantic_lane":false,"supersession_demotion":false,"supersession_demotion_weight":1.5,"verbatim_spans":false}`
- Embedding posture: `{"dimensions":1536,"model":"straylight-hashing-v1","provider":"hashing","status":"degraded"}`

## Results
| Condition | Cases | Claims | Mean cumulative input | Mean cached input | Mean uncached input | Mean shell calls | Mean workspace calls |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Native Straylight checkpoint resume | 0/5 | 13/20 | 77,845 | 52,736 | 25,109 | 2.4 | 2.4 |
| Filesystem rebuild | 0/5 | 12/20 | 87,251 | 65,741 | 21,511 | 3.4 | n/a |
| Filesystem rebuild with writable sidecar | 0/5 | 12/20 | 136,236 | 110,848 | 25,388 | 4.0 | 0.0 |

## Per-card results
| Card | Native Straylight checkpoint resume | Filesystem rebuild | Filesystem rebuild with writable sidecar |
| --- | ---: | ---: | ---: |
| `warmind-alpha-budget-transition` | FAIL 3/4 | FAIL 3/4 | FAIL 2/4 |
| `charlemagne-storage-observation-transition` | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `star-rupture-second-facturer-transition` | FAIL 1/4 | FAIL 1/4 | FAIL 2/4 |
| `switzerland-reservation-transition` | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `straylight-api-gate-transition` | FAIL 3/4 | FAIL 2/4 | FAIL 2/4 |

## Findings
- Native Straylight checkpoint resume returned 35,075 model-visible service characters per card: 23,352 evidence text and 11,723 transport metadata. Replay-weighted output was 83,060 characters.
- Filesystem reconstruction recovered 12/20 claims; Native Straylight checkpoint resume recovered 13/20.
- Checkpoint resume reduced mean cumulative input by 11%, uncached input by -17%, and shell calls by 29% versus rebuilding from the filesystem.
- Native Straylight checkpoint resume committed 5/5 immutable child checkpoints with exact parent, pinned input revision, prior-source, and delta-source lineage.
- Four cards completed with `open -> checkpoint`; one used `open -> read -> checkpoint`. Mean service calls were 2.4, below the four-call gate.
- Mean elapsed time was 60.8s for checkpoint resume versus 57.6s for filesystem reconstruction.
- This draw used `--embeddings none` against the workerless hashing/degraded runtime. No OpenAI embedding index was built or billed; agents issued 1 lexical query call.

## Gate
- Checkpoint-resume call budget: no more than 4 service calls per card.
- Resume pass additionally requires a session pinned to revision N+1, an immutable child checkpoint with exact parent linkage, and both prior and delta source references.
- Native resume additionally requires the committed child checkpoint to be read back through the HTTP session/checkpoint surface.
- Synthetic deltas are isolated evaluation fixtures and are not project, production, game, or booking truth.
- No pre-fix or partial transition run is included in the E01 definitive aggregate.

## Reproduce
```bash
cd /Users/Shared/projects/straylight
python3 transition_eval.py validate
STRAYLIGHT_API_URL=http://127.0.0.1:18110 STRAYLIGHT_EVAL_TOKEN='<owner read/write token>' python3 transition_eval.py run --filesystem-native --concurrency 2 --timeout 420 --out results/native-transitions.json
```

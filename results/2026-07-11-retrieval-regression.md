Created: 2026-07-11T00:23:48-07:00
Updated: 2026-07-11T00:23:48-07:00
Status: Complete

Related: [[Straylight]], [[Retrieval API - Initial Design]], [[Projects/Straylight/Retrieval evaluation plan|Retrieval evaluation plan]]

# Straylight retrieval evaluation results - 2026-07-10

## Scope
- Corpus: 44 Markdown documents, 334,218 characters, 787 chunks
- Frozen corpus root: `/Users/Shared/projects/straylight/eval/corpus-v0.1`
- Cases: 20 frozen questions with 53 gold evidence items
- Manifest SHA-256: `050fa6041457bb63f43b0f1d67f549a2b180f7b1135ea2897d2f6c743425bf9e`
- Corpus SHA-256: `7e92b3cc21ff20c80964e89d84eb44aad0bb38b4b7d5fe70105850ce8455c1bf`
- Retrieval policy: `0.2`
- Harness SHA-256: `b7fe217136adaacd1f07fc36ae61dca65ccd59683293e609adcb06b529663373`
- Corpus areas: Metis, N24 RaceWatch, Home Network Improvements, and Straylight
- Private, health, finance, family, and work-record folders were excluded.

## What this run measures
This is a deterministic retrieval-readiness benchmark. A case passes when every frozen gold evidence item needed to answer the question is present in the returned material. It measures answerability and evidence coverage, not prose quality from a separately sampled language model.

## Results
| Method | Cases passed | Evidence recall | Median chars | P95 chars | Mean estimated tokens |
| --- | ---: | ---: | ---: | ---: | ---: |
| Direct filesystem | 17/20 (85%) | 47/53 (89%) | 29,435 | 44,629 | 7,666 |
| One-shot top-k | 17/20 (85%) | 48/53 (91%) | 4,273 | 5,594 | 1,070 |
| Memory Workspace | 17/20 (85%) | 46/53 (87%) | 11,909 | 11,999 | 2,932 |

## Tuning pass
| Workspace run | Cases passed | Evidence recall | Median chars | P95 chars | Mean estimated tokens |
| --- | ---: | ---: | ---: | ---: | ---: |
| Initial policy | 12/20 (60%) | 39/53 (74%) | 11,866 | 47,995 | 4,309 |
| Policy 0.2 | 17/20 (85%) | 46/53 (87%) | 11,909 | 11,999 | 2,932 |
- Frozen-input check: PASS; manifest and corpus hashes match the initial run.
- Policy changes: compact project maps, max-weight project routing, clause follow-up queries, and breadth-first admission before expansion.
- This tuning used the initial failure set. The improved score is a regression result, not holdout evidence.

## Workspace delta
- Cases recovered beyond one-shot top-k: `metis-storage-policy`, `n24-production-source`
- Cases lost relative to one-shot top-k: `home-core-switch`, `n24-timing-reuse`

## Per-case results
| Case | Category | Direct | One-shot | Workspace | One-shot chars | Workspace chars |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `home-cabling-proof` | verification | PASS | PASS | PASS | 5,594 | 11,999 |
| `home-core-switch` | multi_fact | PASS | PASS | 0/4 | 5,838 | 11,983 |
| `home-gateway-storage` | exact_fact | PASS | PASS | PASS | 4,620 | 11,811 |
| `home-nyx-ethernet` | contradiction | PASS | PASS | PASS | 4,179 | 11,802 |
| `metis-dream-promotion` | policy | PASS | PASS | PASS | 4,858 | 10,584 |
| `metis-ocr-model-fallback` | exact_fact | PASS | PASS | PASS | 4,300 | 11,926 |
| `metis-scanned-bill-status` | temporal_state | PASS | PASS | PASS | 3,620 | 11,949 |
| `metis-session-corpus` | multi_fact | PASS | PASS | PASS | 4,246 | 11,079 |
| `metis-storage-policy` | policy | 0/2 | 0/2 | PASS | 2,718 | 11,999 |
| `metis-sync-limits` | exact_fact | PASS | PASS | PASS | 3,960 | 11,899 |
| `n24-fastn24-inspiration` | architecture | PASS | PASS | PASS | 3,208 | 11,858 |
| `n24-next-event-config` | continuation | PASS | PASS | PASS | 4,559 | 11,995 |
| `n24-production-source` | multi_source | PASS | 1/2 | PASS | 4,543 | 10,266 |
| `n24-timing-reuse` | multi_hop | PASS | PASS | 1/2 | 5,098 | 11,796 |
| `n24-websocket-load` | quantitative | PASS | PASS | PASS | 3,759 | 11,956 |
| `straylight-directional-trust` | trust | PASS | PASS | PASS | 5,165 | 11,917 |
| `straylight-dream-authority` | authority | 0/2 | PASS | PASS | 4,784 | 11,869 |
| `straylight-dream-output-policy` | authority | 0/2 | 0/2 | 0/2 | 3,503 | 11,901 |
| `straylight-retrieval-priority` | multi_hop | PASS | PASS | PASS | 3,760 | 11,995 |
| `straylight-save-stage` | api_contract | PASS | PASS | PASS | 3,286 | 11,959 |

## Failures to inspect
- Direct filesystem: `metis-storage-policy`, `straylight-dream-authority`, `straylight-dream-output-policy`
- One-shot top-k: `metis-storage-policy`, `n24-production-source`, `straylight-dream-output-policy`
- Memory Workspace: `n24-timing-reuse`, `home-core-switch`, `straylight-dream-output-policy`

## Interpretation
The workspace tied one-shot retrieval at 17/20 complete cases, but recovered 2 fewer gold evidence items while using 2.7x the mean estimated context. One-shot top-k is the strongest default on this benchmark; the Memory Workspace hypothesis is not yet demonstrated.
The workspace still trails direct filesystem evidence coverage, so the retrieval contract is not yet a replacement for direct source access.

## Limitations
- The same author selected the corpus and gold evidence, so this is an engineering regression test, not an independent scientific evaluation.
- Policy 0.2 was tuned against the initial run's failures; the final score is not a holdout result.
- Gold matching uses normalized exact evidence strings. It does not grade semantic paraphrases or generated-answer quality.
- The workspace policy is deterministic pseudo-relevance feedback, not a model choosing its own follow-up queries.
- The corpus is project documentation, not raw chat, email, images, or volatile live state.
- Full project materialization is allowed below the frozen 45,000-character threshold and its cost is reported rather than hidden.

## Next decision
Freeze policy 0.2 and run a blinded model-answer evaluation with model-directed workspace calls plus an untouched holdout set. Do not choose the canonical storage engine from this retrieval-only run.

## Reproduce
```bash
cd /Users/Shared/projects/straylight
python3 -m unittest discover -s tests -v
python3 straylight_eval.py validate --vault eval/corpus-v0.1
python3 straylight_eval.py run --vault eval/corpus-v0.1 --baseline results/2026-07-10-v0.1.json --out results/2026-07-10-v0.2.json --report '/Users/aether/obsidian/notes/Projects/Straylight/Retrieval evaluation results - 2026-07-10.md'
```

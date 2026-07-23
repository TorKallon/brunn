Created: 2026-07-22T11:25:05-07:00
Updated: 2026-07-22T11:25:05-07:00
Status: Complete

Related: [[Straylight]], [[Retrieval API - Initial Design]], [[Projects/Straylight/Retrieval evaluation plan|Retrieval evaluation plan]]

# Straylight retrieval evaluation results - 2026-07-10

## Scope
- Corpus: 58 Markdown documents, 541,639 characters, 1101 chunks
- Frozen corpus root: `/Users/aether/obsidian/notes`
- Cases: 20 frozen questions with 53 gold evidence items
- Manifest SHA-256: `050fa6041457bb63f43b0f1d67f549a2b180f7b1135ea2897d2f6c743425bf9e`
- Corpus SHA-256: `e6af36999a63ab22bc2c627ff17259ebbc8391ea6cfef0f6b2995795e4241c9c`
- Retrieval policy: `0.2`
- Harness SHA-256: `b7fe217136adaacd1f07fc36ae61dca65ccd59683293e609adcb06b529663373`
- Corpus areas: Metis, N24 RaceWatch, Home Network Improvements, and Straylight
- Private, health, finance, family, and work-record folders were excluded.

## What this run measures
This is a deterministic retrieval-readiness benchmark. A case passes when every frozen gold evidence item needed to answer the question is present in the returned material. It measures answerability and evidence coverage, not prose quality from a separately sampled language model.

## Results
| Method | Cases passed | Evidence recall | Median chars | P95 chars | Mean estimated tokens |
| --- | ---: | ---: | ---: | ---: | ---: |
| Direct filesystem | 17/20 (85%) | 47/53 (89%) | 33,693 | 43,799 | 8,196 |
| One-shot top-k | 17/20 (85%) | 48/53 (91%) | 4,521 | 5,676 | 1,121 |
| Memory Workspace | 16/20 (80%) | 46/53 (87%) | 11,916 | 11,995 | 2,946 |

## Tuning pass
| Workspace run | Cases passed | Evidence recall | Median chars | P95 chars | Mean estimated tokens |
| --- | ---: | ---: | ---: | ---: | ---: |
| Initial policy | 17/20 (85%) | 46/53 (87%) | 11,909 | 11,999 | 2,932 |
| Policy 0.2 | 16/20 (80%) | 46/53 (87%) | 11,916 | 11,995 | 2,946 |
- Frozen-input check: FAIL; manifest and corpus hashes do not match the initial run.
- Policy changes: compact project maps, max-weight project routing, clause follow-up queries, and breadth-first admission before expansion.
- This tuning used the initial failure set. The improved score is a regression result, not holdout evidence.

## Workspace delta
- Cases recovered beyond one-shot top-k: `n24-production-source`
- Cases lost relative to one-shot top-k: `n24-next-event-config`, `n24-timing-reuse`

## Per-case results
| Case | Category | Direct | One-shot | Workspace | One-shot chars | Workspace chars |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `home-cabling-proof` | verification | PASS | PASS | PASS | 5,676 | 11,933 |
| `home-core-switch` | multi_fact | PASS | PASS | PASS | 5,838 | 11,759 |
| `home-gateway-storage` | exact_fact | PASS | PASS | PASS | 4,228 | 11,905 |
| `home-nyx-ethernet` | contradiction | PASS | PASS | PASS | 4,771 | 11,803 |
| `metis-dream-promotion` | policy | PASS | PASS | PASS | 4,858 | 10,585 |
| `metis-ocr-model-fallback` | exact_fact | PASS | PASS | PASS | 4,300 | 11,927 |
| `metis-scanned-bill-status` | temporal_state | PASS | PASS | PASS | 3,891 | 11,950 |
| `metis-session-corpus` | multi_fact | PASS | PASS | PASS | 4,748 | 10,852 |
| `metis-storage-policy` | policy | 0/2 | 0/2 | 0/2 | 2,718 | 11,977 |
| `metis-sync-limits` | exact_fact | PASS | PASS | PASS | 3,960 | 11,995 |
| `n24-fastn24-inspiration` | architecture | PASS | PASS | PASS | 3,431 | 11,859 |
| `n24-next-event-config` | continuation | PASS | PASS | 2/4 | 4,559 | 11,969 |
| `n24-production-source` | multi_source | PASS | 1/2 | PASS | 4,543 | 11,930 |
| `n24-timing-reuse` | multi_hop | PASS | PASS | 1/2 | 5,098 | 11,797 |
| `n24-websocket-load` | quantitative | PASS | PASS | PASS | 3,759 | 11,969 |
| `straylight-directional-trust` | trust | PASS | PASS | PASS | 5,594 | 11,948 |
| `straylight-dream-authority` | authority | 0/2 | PASS | PASS | 5,464 | 11,752 |
| `straylight-dream-output-policy` | authority | 0/2 | 0/2 | 0/2 | 4,478 | 11,859 |
| `straylight-retrieval-priority` | multi_hop | PASS | PASS | PASS | 3,213 | 11,995 |
| `straylight-save-stage` | api_contract | PASS | PASS | PASS | 4,499 | 11,850 |

## Failures to inspect
- Direct filesystem: `metis-storage-policy`, `straylight-dream-authority`, `straylight-dream-output-policy`
- One-shot top-k: `metis-storage-policy`, `n24-production-source`, `straylight-dream-output-policy`
- Memory Workspace: `metis-storage-policy`, `n24-next-event-config`, `n24-timing-reuse`, `straylight-dream-output-policy`

## Interpretation
The workspace tied one-shot retrieval at 16/20 complete cases, but recovered 2 fewer gold evidence items while using 2.6x the mean estimated context. One-shot top-k is the strongest default on this benchmark; the Memory Workspace hypothesis is not yet demonstrated.
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

Created: 2026-07-26T21:40:13-07:00
Updated: 2026-07-26T21:40:13-07:00
Status: Complete

Related: [[Straylight]], [[Projects/Straylight/Decisions|Decisions]]

# Straylight agent-work evaluation - 2026-07-26

## Scope
- Model: `gpt-5.6-sol`
- Corpus: 73 files, 1,280,331 characters, 1563 chunks
- Corpus SHA-256: `b08ded20cdc2f1437da8cc0db5b217de0f84e89a33814995307fd81681be0bc2`
- Cases: 1 complex work tasks with 4 scored claims
- Workloads: Warmind
- This evaluates agent work and durable checkpoints, not retrieval recall alone.

## Conditions
- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.
- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations.

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Filesystem agent | 1/1 (100%) | 4/4 (100%) | 1.000 | output only |
| Native Straylight API agent | 1/1 (100%) | 4/4 (100%) | 1.000 | 1/1 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Filesystem agent | 156,896 | 115,456 | 41,440 | 2,592 | 4.0 | 1,343,391 |
| Native Straylight API agent | 146,466 | 122,624 | 23,842 | 3,243 | 4.0 | 72,925 |

## Workload results
| Workload | Filesystem agent | Native Straylight API agent |
| --- | ---: | ---: |
| Warmind | 1/1 cases, 4/4 claims | 1/1 cases, 4/4 claims |

## Per-case results
| Case | Capability | Filesystem agent | Native Straylight API agent |
| --- | --- | ---: | ---: |
| `warmind-final-validation-handoff` | durable_handoff | PASS 4/4 | PASS 4/4 |

## Findings
- Highest complete-case rate: Filesystem agent, Native Straylight API agent at 100%.
- Native service calls averaged 3.0 per case and returned 71,988 characters in 207.0 ms of measured API time.
- Of that model-visible service output, 64,061 characters were evidence text and 7,927 were transport metadata; replay-weighted output was 193,178 characters per case.
- Native uncached model input was 0.58x the filesystem baseline.

## Interpretation boundary
This suite uses a single model and deterministic claim rubrics. It tests whether each access surface supports correct, cited work products and durable checkpoints; it does not isolate every possible model-policy interaction.

## Conclusions
- Complex agent work needs recoverable source and artifact access. A fixed handoff is useful orientation, but it cannot be the durable work substrate.
- Direct filesystem access is the quality and efficiency baseline. Straylight must preserve that freedom while adding portable checkpoints, authority, provenance, trust policy, and cross-agent continuity.
- The native API recovered 4/4 claims versus 4/4 for filesystem access and persisted every eligible checkpoint.
- The native condition exercised exact, structured, lexical, semantic, temporal, and relation retrieval with source diversification and authority-preserving checkpoint writes.

## Limitations
- The corpus and rubrics were authored from known project material, so future runs need untouched holdout tasks.
- The fixed pack is a strong task-specific handoff control, not a generic retrieval baseline.
- Cumulative input includes cached-history replay and is not a measure of unique evidence.
- Live telemetry, external websites, and production state were intentionally unavailable.

## Reproduce
```bash
cd /Users/Shared/projects/straylight
python3 -m unittest discover -s tests -v
python3 agent_work_eval.py --manifest eval/work_cases.json validate
python3 agent_work_eval.py --manifest eval/work_cases.json run --filesystem-native --concurrency 3 --timeout 420 --out results/native-agent-work.json
```

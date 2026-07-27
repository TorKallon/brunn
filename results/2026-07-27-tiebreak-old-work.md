Created: 2026-07-27T10:26:46-07:00
Updated: 2026-07-27T10:26:46-07:00
Status: Complete

Related: [[Straylight]], [[Projects/Straylight/Decisions|Decisions]]

# Straylight agent-work evaluation - 2026-07-27

## Scope
- Model: `gpt-5.6-sol`
- Corpus: 73 files, 1,280,331 characters, 1563 chunks
- Corpus SHA-256: `b08ded20cdc2f1437da8cc0db5b217de0f84e89a33814995307fd81681be0bc2`
- Cases: 3 complex work tasks with 12 scored claims
- Workloads: Charlemagne, Switzerland
- This evaluates agent work and durable checkpoints, not retrieval recall alone.

## Conditions
- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations.

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Native Straylight API agent | 1/3 (33%) | 9/12 (75%) | 0.924 | 3/3 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Native Straylight API agent | 131,354 | 101,025 | 30,328 | 3,169 | 4.3 | 50,843 |

## Workload results
| Workload | Native Straylight API agent |
| --- | ---: |
| Charlemagne | 1/1 cases, 4/4 claims |
| Switzerland | 0/2 cases, 5/8 claims |

## Per-case results
| Case | Capability | Native Straylight API agent |
| --- | --- | ---: |
| `charlemagne-storage-current-state` | temporal_state_and_planning | PASS 4/4 |
| `switzerland-resume-truth` | planning_state_and_corrections | FAIL 2/4 |
| `switzerland-authority-and-next-actions` | authority_and_actionability | FAIL 3/4 |

## Findings
- Highest complete-case rate: Native Straylight API agent at 33%.

## Interpretation boundary
This suite uses a single model and deterministic claim rubrics. It tests whether each access surface supports correct, cited work products and durable checkpoints; it does not isolate every possible model-policy interaction.

## Conclusions
- Complex agent work needs recoverable source and artifact access. A fixed handoff is useful orientation, but it cannot be the durable work substrate.
- Direct filesystem access is the quality and efficiency baseline. Straylight must preserve that freedom while adding portable checkpoints, authority, provenance, trust policy, and cross-agent continuity.
- The initial shell interface creates too many model/tool round trips. A native typed API, persistent index and session, batched retrieval, compact deltas, and non-echoing checkpoint writes should remove most of that overhead.
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

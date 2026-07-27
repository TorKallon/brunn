Created: 2026-07-26T18:03:31-07:00
Updated: 2026-07-26T18:20:40-07:00
Status: Complete

Related: [[Straylight]], [[Projects/Straylight/Decisions|Decisions]]

# Straylight agent-work evaluation - 2026-07-26

## Scope
- Model: `gpt-5.6-sol`
- Corpus: 73 files, 1,280,331 characters, 1563 chunks
- Corpus SHA-256: `b08ded20cdc2f1437da8cc0db5b217de0f84e89a33814995307fd81681be0bc2`
- Cases: 13 complex work tasks with 52 scored claims
- Workloads: Charlemagne, Star Rupture, Straylight, Switzerland, Warmind
- This evaluates agent work and durable checkpoints, not retrieval recall alone.

## Conditions
- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.
- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations.

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Filesystem agent | 4/13 (31%) | 43/52 (83%) | 0.962 | output only |
| Native Straylight API agent | 6/13 (46%) | 43/52 (83%) | 0.942 | 13/13 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Filesystem agent | 108,041 | 77,607 | 30,434 | 2,732 | 3.2 | 307,103 |
| Native Straylight API agent | 185,706 | 155,136 | 30,570 | 3,507 | 5.3 | 111,709 |

## Workload results
| Workload | Filesystem agent | Native Straylight API agent |
| --- | ---: | ---: |
| Charlemagne | 1/3 cases, 10/12 claims | 2/3 cases, 11/12 claims |
| Star Rupture | 2/3 cases, 11/12 claims | 2/3 cases, 11/12 claims |
| Straylight | 0/1 cases, 3/4 claims | 1/1 cases, 4/4 claims |
| Switzerland | 0/3 cases, 9/12 claims | 0/3 cases, 8/12 claims |
| Warmind | 1/3 cases, 10/12 claims | 1/3 cases, 9/12 claims |

## Per-case results
| Case | Capability | Filesystem agent | Native Straylight API agent |
| --- | --- | ---: | ---: |
| `warmind-parser-learning` | learning_and_supersession | FAIL 3/4 | FAIL 2/4 |
| `warmind-final-validation-handoff` | durable_handoff | FAIL 3/4 | FAIL 3/4 |
| `charlemagne-storage-current-state` | temporal_state_and_planning | FAIL 3/4 | FAIL 3/4 |
| `charlemagne-performance-priority` | evidence_backed_investigation | PASS 4/4 | PASS 4/4 |
| `warmind-production-cpu-mitigation` | incident_continuation | PASS 4/4 | PASS 4/4 |
| `charlemagne-index-artifact-review` | artifact_computation_and_safety | FAIL 3/4 | PASS 4/4 |
| `star-rupture-rail-and-heat-plan` | quantitative_planning | FAIL 3/4 | PASS 4/4 |
| `star-rupture-source-authority` | authority_and_missing_artifacts | PASS 4/4 | PASS 4/4 |
| `star-rupture-plan-revision` | iterative_workspace_revision | PASS 4/4 | FAIL 3/4 |
| `switzerland-resume-truth` | planning_state_and_corrections | FAIL 3/4 | FAIL 2/4 |
| `switzerland-itinerary-revision` | constraint_driven_iteration | FAIL 3/4 | FAIL 3/4 |
| `switzerland-authority-and-next-actions` | authority_and_actionability | FAIL 3/4 | FAIL 3/4 |
| `straylight-product-resume` | project_continuation | FAIL 3/4 | PASS 4/4 |

## Findings
- Highest complete-case rate: Native Straylight API agent at 46%.
- Native service calls averaged 4.5 per case and returned 110,834 characters in 423.9 ms of measured API time.
- Of that model-visible service output, 102,325 characters were evidence text and 8,510 were transport metadata; replay-weighted output was 464,383 characters per case.
- Native uncached model input was 1.00x the filesystem baseline.

## Interpretation boundary
This suite uses a single model and deterministic claim rubrics. It tests whether each access surface supports correct, cited work products and durable checkpoints; it does not isolate every possible model-policy interaction.

## Conclusions
- Complex agent work needs recoverable source and artifact access. A fixed handoff is useful orientation, but it cannot be the durable work substrate.
- Direct filesystem access is the quality and efficiency baseline. Straylight must preserve that freedom while adding portable checkpoints, authority, provenance, trust policy, and cross-agent continuity.
- The native API recovered 43/52 claims versus 43/52 for filesystem access and persisted every eligible checkpoint.
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

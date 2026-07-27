Created: 2026-07-26T18:35:58-07:00
Updated: 2026-07-26T18:39:34-07:00
Status: Complete

Related: [[Straylight]], [[Projects/Straylight/Decisions|Decisions]]

# Straylight Rupture Ops workload evaluation - 2026-07-11

## Scope
- Model: `gpt-5.6-sol`
- Corpus: 65 files, 468,399 characters, 587 chunks
- Corpus SHA-256: `aa9c33e39777f0899dacecee61a94f5fd11ec1315f7a6e820a41bc217e1a9803`
- Cases: 12 complex work tasks with 48 scored claims
- Workloads: RuptureOps, StarRupture
- This evaluates agent work and durable checkpoints, not retrieval recall alone.

## Conditions
- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.
- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations.

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Filesystem agent | 4/12 (33%) | 36/48 (75%) | 0.950 | output only |
| Native Straylight API agent | 3/12 (25%) | 37/48 (77%) | 0.952 | 12/12 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Filesystem agent | 149,017 | 111,787 | 37,230 | 3,182 | 5.1 | 142,134 |
| Native Straylight API agent | 171,828 | 139,029 | 32,799 | 3,555 | 4.8 | 123,977 |

## Workload results
| Workload | Filesystem agent | Native Straylight API agent |
| --- | ---: | ---: |
| RuptureOps | 1/7 cases, 18/28 claims | 2/7 cases, 22/28 claims |
| StarRupture | 3/5 cases, 18/20 claims | 1/5 cases, 15/20 claims |

## Per-case results
| Case | Capability | Filesystem agent | Native Straylight API agent |
| --- | --- | ---: | ---: |
| `ruptureops-archive-import-reconciliation` | bulk_import_and_provenance | FAIL 2/4 | FAIL 3/4 |
| `ruptureops-prompt-history-reconciliation` | overlapping_history_and_epistemic_state | FAIL 3/4 | FAIL 3/4 |
| `ruptureops-save-state-truth` | typed_current_historical_and_planned_state | PASS 4/4 | FAIL 2/4 |
| `ruptureops-live-poi-advice` | situated_low_latency_decision_support | FAIL 3/4 | FAIL 3/4 |
| `ruptureops-geo-scanner-learning` | field_observation_to_durable_learning | PASS 4/4 | PASS 4/4 |
| `ruptureops-spatial-evidence` | multimodal_spatial_and_coordinate_reasoning | FAIL 3/4 | FAIL 3/4 |
| `ruptureops-flowworks-campaign-revision` | quantitative_plan_with_supersession | FAIL 3/4 | FAIL 3/4 |
| `ruptureops-multi-goal-field-plan` | multi_goal_continuation_and_compression | PASS 4/4 | FAIL 3/4 |
| `ruptureops-watch-session-design` | research_to_product_specification | PASS 4/4 | PASS 4/4 |
| `ruptureops-interrupted-ios-continuation` | partial_code_and_acceptance_gate_recovery | FAIL 3/4 | FAIL 3/4 |
| `ruptureops-private-public-artifact-boundary` | artifact_policy_and_export_safety | FAIL 2/4 | PASS 4/4 |
| `ruptureops-forked-agent-idempotency` | parallel_agent_coordination_and_idempotency | FAIL 1/4 | FAIL 2/4 |

## Findings
- Highest complete-case rate: Filesystem agent at 33%.
- Native service calls averaged 4.2 per case and returned 123,418 characters in 130.6 ms of measured API time.
- Of that model-visible service output, 113,024 characters were evidence text and 10,394 were transport metadata; replay-weighted output was 470,073 characters per case.
- Native uncached model input was 0.88x the filesystem baseline.

## Interpretation boundary
This suite uses a single model and deterministic claim rubrics. It tests whether each access surface supports correct, cited work products and durable checkpoints; it does not isolate every possible model-policy interaction.

## Conclusions
- Complex agent work needs recoverable source and artifact access. A fixed handoff is useful orientation, but it cannot be the durable work substrate.
- Direct filesystem access is the quality and efficiency baseline. Straylight must preserve that freedom while adding portable checkpoints, authority, provenance, trust policy, and cross-agent continuity.
- The native API recovered 37/48 claims versus 36/48 for filesystem access and persisted every eligible checkpoint.
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
python3 agent_work_eval.py --manifest eval/rupture_ops_cases.json validate
python3 agent_work_eval.py --manifest eval/rupture_ops_cases.json run --filesystem-native --concurrency 3 --timeout 420 --out results/native-agent-work.json
```

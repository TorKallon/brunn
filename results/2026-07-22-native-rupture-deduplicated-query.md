Created: 2026-07-22T13:09:36-07:00
Updated: 2026-07-22T13:09:36-07:00
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
- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations.

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Native Straylight API agent | 6/12 (50%) | 40/48 (83%) | 0.888 | 12/12 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Native Straylight API agent | 177,105 | 139,520 | 37,585 | 3,690 | 4.8 | 96,765 |

## Workload results
| Workload | Native Straylight API agent |
| --- | ---: |
| RuptureOps | 2/7 cases, 21/28 claims |
| StarRupture | 4/5 cases, 19/20 claims |

## Per-case results
| Case | Capability | Native Straylight API agent |
| --- | --- | ---: |
| `ruptureops-archive-import-reconciliation` | bulk_import_and_provenance | FAIL 2/4 |
| `ruptureops-prompt-history-reconciliation` | overlapping_history_and_epistemic_state | PASS 4/4 |
| `ruptureops-save-state-truth` | typed_current_historical_and_planned_state | FAIL 3/4 |
| `ruptureops-live-poi-advice` | situated_low_latency_decision_support | PASS 4/4 |
| `ruptureops-geo-scanner-learning` | field_observation_to_durable_learning | PASS 4/4 |
| `ruptureops-spatial-evidence` | multimodal_spatial_and_coordinate_reasoning | PASS 4/4 |
| `ruptureops-flowworks-campaign-revision` | quantitative_plan_with_supersession | PASS 4/4 |
| `ruptureops-multi-goal-field-plan` | multi_goal_continuation_and_compression | PASS 4/4 |
| `ruptureops-watch-session-design` | research_to_product_specification | FAIL 2/4 |
| `ruptureops-interrupted-ios-continuation` | partial_code_and_acceptance_gate_recovery | FAIL 3/4 |
| `ruptureops-private-public-artifact-boundary` | artifact_policy_and_export_safety | FAIL 3/4 |
| `ruptureops-forked-agent-idempotency` | parallel_agent_coordination_and_idempotency | FAIL 3/4 |

## Findings
- Highest complete-case rate: Native Straylight API agent at 50%.

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
python3 agent_work_eval.py --manifest eval/rupture_ops_cases.json validate
python3 agent_work_eval.py --manifest eval/rupture_ops_cases.json run --filesystem-native --concurrency 3 --timeout 420 --out results/native-agent-work.json
```

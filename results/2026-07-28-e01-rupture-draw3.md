Created: 2026-07-28T04:05:40-07:00
Updated: 2026-07-28T04:05:40-07:00
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
- Service build revision: `232e7c6c8b8675d3ceeee3ff02a600ffdb95d2da`
- Expected runtime features: `{"intention_ledger":false,"lexical_single_scan":false,"read_path_roundtrip_v1":false,"resume_deltas":false,"search_char_cap":false,"search_fair_share":false,"search_section_demotion_top_n":null,"search_top1_hydration":false,"semantic_lane":false,"supersession_demotion":false,"verbatim_spans":false}`
- Actual runtime features: `{"allow_degraded_embeddings":true,"embed_cache":true,"embedding_backfill_batch_chunks":64,"embedding_backfill_foreground_status_timeout_ms":1000,"embedding_backfill_foreground_status_url_configured":true,"embedding_backfill_guard":true,"embedding_backfill_inter_batch_ms":250,"embedding_backfill_open_p95_limit_ms":120.0,"embedding_backfill_search_p95_limit_ms":107.0,"intention_ledger":false,"lexical_single_scan":false,"materialize_token_budget":24000,"observability_timings_ms":true,"read_path_roundtrip_v1":false,"resume_deltas":false,"search_char_cap":false,"search_fair_share":false,"search_section_demotion_top_n":null,"search_top1_hydration":false,"semantic_deadline_ms":300,"semantic_lane":false,"supersession_demotion":false,"supersession_demotion_weight":1.5,"verbatim_spans":false}`
- Embedding posture: `{"dimensions":1536,"model":"straylight-hashing-v1","provider":"hashing","status":"degraded"}`

## Conditions
- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations.
- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.
- **Filesystem agent with writable sidecar:** the corpus remains read-only, while a run-scoped sidecar must receive a durable JSON checkpoint.

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Native Straylight API agent | 4/12 (33%) | 35/48 (73%) | 0.943 | 12/12 eligible |
| Filesystem agent | 5/12 (42%) | 37/48 (77%) | 0.946 | output only |
| Filesystem agent with writable sidecar | 5/12 (42%) | 39/48 (81%) | 0.956 | 12/12 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Native Straylight API agent | 134,430 | 108,736 | 25,694 | 3,155 | 3.8 | 60,519 |
| Filesystem agent | 143,760 | 109,653 | 34,106 | 3,078 | 4.8 | 126,902 |
| Filesystem agent with writable sidecar | 204,426 | 164,693 | 39,733 | 4,100 | 4.8 | 113,961 |

## Workload results
| Workload | Native Straylight API agent | Filesystem agent | Filesystem agent with writable sidecar |
| --- | ---: | ---: | ---: |
| RuptureOps | 2/7 cases, 19/28 claims | 2/7 cases, 19/28 claims | 3/7 cases, 22/28 claims |
| StarRupture | 2/5 cases, 16/20 claims | 3/5 cases, 18/20 claims | 2/5 cases, 17/20 claims |

## Per-case results
| Case | Capability | Native Straylight API agent | Filesystem agent | Filesystem agent with writable sidecar |
| --- | --- | ---: | ---: | ---: |
| `ruptureops-archive-import-reconciliation` | bulk_import_and_provenance | FAIL 3/4 | FAIL 2/4 | FAIL 2/4 |
| `ruptureops-prompt-history-reconciliation` | overlapping_history_and_epistemic_state | FAIL 2/4 | FAIL 3/4 | PASS 4/4 |
| `ruptureops-save-state-truth` | typed_current_historical_and_planned_state | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `ruptureops-live-poi-advice` | situated_low_latency_decision_support | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `ruptureops-geo-scanner-learning` | field_observation_to_durable_learning | FAIL 2/4 | PASS 4/4 | FAIL 3/4 |
| `ruptureops-spatial-evidence` | multimodal_spatial_and_coordinate_reasoning | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `ruptureops-flowworks-campaign-revision` | quantitative_plan_with_supersession | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `ruptureops-multi-goal-field-plan` | multi_goal_continuation_and_compression | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `ruptureops-watch-session-design` | research_to_product_specification | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `ruptureops-interrupted-ios-continuation` | partial_code_and_acceptance_gate_recovery | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `ruptureops-private-public-artifact-boundary` | artifact_policy_and_export_safety | FAIL 1/4 | FAIL 2/4 | FAIL 2/4 |
| `ruptureops-forked-agent-idempotency` | parallel_agent_coordination_and_idempotency | FAIL 2/4 | FAIL 1/4 | FAIL 3/4 |

## Findings
- Highest complete-case rate: Filesystem agent, Filesystem agent with writable sidecar at 42%.
- Native service calls averaged 3.1 per case and returned 59,884 characters in 166.0 ms of measured API time.
- Of that model-visible service output, 44,545 characters were evidence text and 15,339 were transport metadata; replay-weighted output was 168,115 characters per case.
- Native uncached model input was 0.75x the filesystem baseline.

## Interpretation boundary
This suite uses a single model and deterministic claim rubrics. It tests whether each access surface supports correct, cited work products and durable checkpoints; it does not isolate every possible model-policy interaction.

## Conclusions
- Complex agent work needs recoverable source and artifact access. A fixed handoff is useful orientation, but it cannot be the durable work substrate.
- Direct filesystem access is the quality and efficiency baseline. Straylight must preserve that freedom while adding portable checkpoints, authority, provenance, trust policy, and cross-agent continuity.
- The native API recovered 35/48 claims versus 37/48 for filesystem access and persisted every eligible checkpoint.
- The native condition exercised exact, structured, lexical, temporal, and relation retrieval with source diversification and authority-preserving checkpoint writes. Semantic retrieval was disabled for E01.

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

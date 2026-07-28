Created: 2026-07-28T15:59:15-07:00
Updated: 2026-07-28T15:59:15-07:00
Status: Complete

Related: [[Straylight]], [[Projects/Straylight/Decisions|Decisions]]

# Straylight recent work and Aether operations evaluation - 2026-07-26

## Scope
- Model: `gpt-5.6-sol`
- Corpus: 28 files, 15,907 characters, 41 chunks
- Corpus SHA-256: `6bc83dbf4366fc3a716799ba300c32f841f9f644dfcc497b2aea0e138ddcb10b`
- Cases: 14 complex work tasks with 56 scored claims
- Workloads: Recent Work
- This evaluates agent work and durable checkpoints, not retrieval recall alone.
- Experiment arm: `e07-filesystem`
- Paired draw ID: `e07-draw1`

## Conditions
- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Filesystem agent | 5/14 (36%) | 41/56 (73%) | 0.913 | output only |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Filesystem agent | 74,774 | 60,709 | 14,065 | 1,538 | 3.1 | 8,312 |

## Workload results
| Workload | Filesystem agent |
| --- | ---: |
| Recent Work | 5/14 cases, 41/56 claims |

## Per-case results
| Case | Capability | Filesystem agent |
| --- | --- | ---: |
| `recent-europe-source-authority` | source_authority_and_itinerary_reconciliation | PASS 4/4 |
| `recent-europe-calendar-dedup` | calendar_deduplication_and_truthful_action_reporting | FAIL 2/4 |
| `recent-europe-rail-resume` | checkpoint_resume_and_consequential_action_gate | FAIL 3/4 |
| `recent-europe-corrections` | owner_correction_and_external_action_state | FAIL 2/4 |
| `recent-tracker-no-delta` | quiet_monitoring_and_delta_suppression | PASS 4/4 |
| `recent-tracker-material-delta` | material_delta_detection_and_baseline_advance | FAIL 2/4 |
| `recent-aether-heartbeat-healthy` | deterministic_healthy_heartbeat | PASS 4/4 |
| `recent-aether-heartbeat-failure` | bounded_failure_heartbeat | PASS 4/4 |
| `recent-aether-gmail-no-action` | quiet_deterministic_intake | FAIL 2/4 |
| `recent-aether-gmail-actions` | action_boundary_dedup_and_attachment_preservation | FAIL 3/4 |
| `recent-aether-morning-brief` | freshness_aware_briefing_and_delivery_gate | FAIL 2/4 |
| `recent-current-over-history` | current_truth_over_historical_checkpoint | FAIL 3/4 |
| `recent-cross-agent-intention` | cross_agent_prospective_memory | PASS 4/4 |
| `recent-intention-expiry-negative` | prospective_expiry_and_completion_boundary | FAIL 2/4 |

## Findings
- Highest complete-case rate: Filesystem agent at 36%.

## Interpretation boundary
This suite uses a single model and deterministic claim rubrics. It tests whether each access surface supports correct, cited work products and durable checkpoints; it does not isolate every possible model-policy interaction.

## Conclusions
- Complex agent work needs recoverable source and artifact access. A fixed handoff is useful orientation, but it cannot be the durable work substrate.
- Direct filesystem access is the quality and efficiency baseline. Straylight must preserve that freedom while adding portable checkpoints, authority, provenance, trust policy, and cross-agent continuity.
- The initial shell interface creates too many model/tool round trips. A native typed API, persistent index and session, batched retrieval, compact deltas, and non-echoing checkpoint writes should remove most of that overhead.
- The current workspace uses lexical BM25 retrieval. Semantic retrieval, reranking, authority and supersession signals, hit rate, and search latency remain untested product hypotheses.

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
python3 agent_work_eval.py --manifest eval/work_cases.json run --concurrency 3 --timeout 420 --out results/native-agent-work.json
```

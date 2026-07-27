Created: 2026-07-26T20:41:36-07:00
Updated: 2026-07-26T20:41:43-07:00
Status: Complete

Related: [[Straylight]], [[Projects/Straylight/Decisions|Decisions]]

# Straylight recent work and Aether operations evaluation - 2026-07-26

## Scope
- Model: `gpt-5.6-sol`
- Corpus: 17 files, 11,327 characters, 20 chunks
- Corpus SHA-256: `1e4f1a1da7c87189d20d8ed01b228eee08058bb5c45afffca62d1dd40518528c`
- Cases: 12 complex work tasks with 48 scored claims
- Workloads: Recent Work
- This evaluates agent work and durable checkpoints, not retrieval recall alone.

## Conditions
- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.
- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations.

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Filesystem agent | 3/12 (25%) | 37/48 (77%) | 0.918 | output only |
| Native Straylight API agent | 4/12 (33%) | 35/48 (73%) | 0.898 | 12/12 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Filesystem agent | 68,145 | 50,347 | 17,798 | 1,517 | 2.7 | 7,359 |
| Native Straylight API agent | 89,551 | 68,544 | 21,007 | 1,912 | 3.3 | 14,825 |

## Workload results
| Workload | Filesystem agent | Native Straylight API agent |
| --- | ---: | ---: |
| Recent Work | 3/12 cases, 37/48 claims | 4/12 cases, 35/48 claims |

## Per-case results
| Case | Capability | Filesystem agent | Native Straylight API agent |
| --- | --- | ---: | ---: |
| `recent-europe-source-authority` | source_authority_and_itinerary_reconciliation | PASS 4/4 | PASS 4/4 |
| `recent-europe-calendar-dedup` | calendar_deduplication_and_truthful_action_reporting | FAIL 3/4 | FAIL 1/4 |
| `recent-europe-rail-resume` | checkpoint_resume_and_consequential_action_gate | PASS 4/4 | PASS 4/4 |
| `recent-europe-corrections` | owner_correction_and_external_action_state | FAIL 3/4 | FAIL 2/4 |
| `recent-tracker-no-delta` | quiet_monitoring_and_delta_suppression | FAIL 3/4 | PASS 4/4 |
| `recent-tracker-material-delta` | material_delta_detection_and_baseline_advance | FAIL 3/4 | FAIL 3/4 |
| `recent-aether-heartbeat-healthy` | deterministic_healthy_heartbeat | FAIL 3/4 | FAIL 3/4 |
| `recent-aether-heartbeat-failure` | bounded_failure_heartbeat | PASS 4/4 | PASS 4/4 |
| `recent-aether-gmail-no-action` | quiet_deterministic_intake | FAIL 2/4 | FAIL 3/4 |
| `recent-aether-gmail-actions` | action_boundary_dedup_and_attachment_preservation | FAIL 3/4 | FAIL 1/4 |
| `recent-aether-morning-brief` | freshness_aware_briefing_and_delivery_gate | FAIL 2/4 | FAIL 3/4 |
| `recent-current-over-history` | current_truth_over_historical_checkpoint | FAIL 3/4 | FAIL 3/4 |

## Findings
- Highest complete-case rate: Native Straylight API agent at 33%.
- Native service calls averaged 2.7 per case and returned 14,200 characters in 96.1 ms of measured API time.
- Of that model-visible service output, 8,756 characters were evidence text and 5,444 were transport metadata; replay-weighted output was 35,929 characters per case.
- Native uncached model input was 1.18x the filesystem baseline.

## Interpretation boundary
This suite uses a single model and deterministic claim rubrics. It tests whether each access surface supports correct, cited work products and durable checkpoints; it does not isolate every possible model-policy interaction.

## Conclusions
- Complex agent work needs recoverable source and artifact access. A fixed handoff is useful orientation, but it cannot be the durable work substrate.
- Direct filesystem access is the quality and efficiency baseline. Straylight must preserve that freedom while adding portable checkpoints, authority, provenance, trust policy, and cross-agent continuity.
- The native API recovered 35/48 claims versus 37/48 for filesystem access and persisted every eligible checkpoint.
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

Created: 2026-07-11T00:12:35-07:00
Updated: 2026-07-11T00:12:35-07:00
Status: Complete

Related: [[Straylight]], [[Projects/Straylight/Decisions|Decisions]]

# Straylight agent-work evaluation - 2026-07-10

## Scope
- Model: `gpt-5.6-sol`
- Corpus: 73 files, 1,280,331 characters, 1563 chunks
- Corpus SHA-256: `b08ded20cdc2f1437da8cc0db5b217de0f84e89a33814995307fd81681be0bc2`
- Cases: 14 complex work tasks with 56 scored claims
- Workloads: Warmind, Charlemagne, Star Rupture, Switzerland, and Straylight
- This evaluates agent work and durable checkpoints, not retrieval recall alone.

## Conditions
- **Fixed handoff pack:** a fresh agent receives one task-specific context file and cannot retrieve more.
- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.
- **Straylight workspace agent:** a fresh agent uses only `open`, `query`, `read`, `compute`, `verify`, and `checkpoint` operations.

## Results
| Condition | Cases passed | Claims passed | Mean score | Mean input tokens | Mean output tokens | Mean evidence chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Fixed handoff pack | 7/14 (50%) | 40/56 (71%) | 0.853 | 36,994 | 1,729 | 16,073 |
| Filesystem agent | 14/14 (100%) | 56/56 (100%) | 0.958 | 97,420 | 2,417 | 0 |
| Straylight workspace agent | 14/14 (100%) | 56/56 (100%) | 0.952 | 210,155 | 3,571 | 37,779 |

## Workload results
| Workload | Fixed pack | Filesystem | Workspace |
| --- | ---: | ---: | ---: |
| Charlemagne | 2/3 cases, 9/12 claims | 3/3 cases, 12/12 claims | 3/3 cases, 12/12 claims |
| Star Rupture | 1/3 cases, 4/12 claims | 3/3 cases, 12/12 claims | 3/3 cases, 12/12 claims |
| Straylight | 2/2 cases, 8/8 claims | 2/2 cases, 8/8 claims | 2/2 cases, 8/8 claims |
| Switzerland | 1/3 cases, 10/12 claims | 3/3 cases, 12/12 claims | 3/3 cases, 12/12 claims |
| Warmind | 1/3 cases, 9/12 claims | 3/3 cases, 12/12 claims | 3/3 cases, 12/12 claims |

## Per-case results
| Case | Capability | Fixed pack | Filesystem | Workspace |
| --- | --- | ---: | ---: | ---: |
| `warmind-parser-learning` | learning_and_supersession | FAIL 2/4 | PASS 4/4 | PASS 4/4 |
| `warmind-final-validation-handoff` | durable_handoff | FAIL 3/4 | PASS 4/4 | PASS 4/4 |
| `charlemagne-storage-current-state` | temporal_state_and_planning | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `charlemagne-performance-priority` | evidence_backed_investigation | FAIL 1/4 | PASS 4/4 | PASS 4/4 |
| `warmind-production-cpu-mitigation` | incident_continuation | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `charlemagne-index-artifact-review` | artifact_computation_and_safety | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `star-rupture-rail-and-heat-plan` | quantitative_planning | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `star-rupture-source-authority` | authority_and_missing_artifacts | FAIL 0/4 | PASS 4/4 | PASS 4/4 |
| `star-rupture-plan-revision` | iterative_workspace_revision | FAIL 0/4 | PASS 4/4 | PASS 4/4 |
| `switzerland-resume-truth` | planning_state_and_corrections | FAIL 3/4 | PASS 4/4 | PASS 4/4 |
| `switzerland-itinerary-revision` | constraint_driven_iteration | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `switzerland-authority-and-next-actions` | authority_and_actionability | FAIL 3/4 | PASS 4/4 | PASS 4/4 |
| `straylight-product-resume` | project_continuation | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `straylight-trust-handoff` | trust_aware_workspace_design | PASS 4/4 | PASS 4/4 | PASS 4/4 |

## Interpretation boundary
The highest complete-case rate in this run was Filesystem agent at 100%.
This suite uses a single model and deterministic claim rubrics. It tests whether each access surface supports correct, cited work products and durable checkpoints; it does not isolate every possible model-policy interaction.

## Limitations
- The corpus and rubrics were authored from known project material, so future runs need untouched holdout tasks.
- The fixed pack is a strong task-specific handoff control, not a generic retrieval baseline.
- The filesystem and workspace agents can choose their own evidence path, so token and latency differences include tool-policy behavior.
- Deterministic claim checks still miss some valid paraphrases; individual failures require answer inspection.
- Live telemetry, external websites, and production state were intentionally unavailable. Agents had to preserve the distinction between saved evidence and current truth.

## Reproduce
```bash
cd /Users/Shared/projects/straylight
python3 -m unittest discover -s tests -v
python3 agent_work_eval.py validate
python3 agent_work_eval.py run --concurrency 3 --out results/2026-07-10-agent-work-v0.2.json --report '/Users/aether/obsidian/notes/Projects/Straylight/Agent work evaluation results - 2026-07-10.md'
```

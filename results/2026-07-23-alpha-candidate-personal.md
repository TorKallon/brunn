Created: 2026-07-23T17:42:28-07:00
Updated: 2026-07-23T17:42:28-07:00
Status: Complete

Related: [[Straylight]], [[Projects/Straylight/Decisions|Decisions]]

# Straylight personal coordination evaluation - 2026-07-11

## Scope
- Model: `gpt-5.6-sol`
- Corpus: 29 files, 29,352 characters, 36 chunks
- Corpus SHA-256: `1f2d62e8f27d2309bdb9353ff349277e038a58b4753c6f3199fd608e9c97ff18`
- Cases: 15 complex work tasks with 60 scored claims
- Workloads: Personal Coordination
- This evaluates agent work and durable checkpoints, not retrieval recall alone.

## Conditions
- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.
- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations. Read-only credentials cannot persist a checkpoint.

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Filesystem agent | 15/15 (100%) | 60/60 (100%) | 0.969 | output only |
| Native Straylight API agent | 15/15 (100%) | 60/60 (100%) | 0.980 | 14/14 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Filesystem agent | 83,612 | 67,823 | 15,790 | 1,863 | 4.4 | 17,135 |
| Native Straylight API agent | 91,295 | 70,656 | 20,639 | 2,228 | 3.2 | 19,976 |

## Workload results
| Workload | Filesystem agent | Native Straylight API agent |
| --- | ---: | ---: |
| Personal Coordination | 15/15 cases, 60/60 claims | 15/15 cases, 60/60 claims |

## Per-case results
| Case | Capability | Filesystem agent | Native Straylight API agent |
| --- | --- | ---: | ---: |
| `coord-person-resolution` | identity_resolution_with_provenance | PASS 4/4 | PASS 4/4 |
| `coord-identity-equivalence-reversal` | reversible_identity_equivalence | PASS 4/4 | PASS 4/4 |
| `coord-person-dossier` | derived_person_view | PASS 4/4 | PASS 4/4 |
| `coord-role-relationship-provenance` | qualified_role_relation_and_authority | PASS 4/4 | PASS 4/4 |
| `coord-canonical-contract-normalization` | canonical_contract_normalization | PASS 4/4 | PASS 4/4 |
| `coord-series-exceptions` | recurring_event_identity_and_exceptions | PASS 4/4 | PASS 4/4 |
| `coord-schedule-supersession` | authoritative_temporal_supersession | PASS 4/4 | PASS 4/4 |
| `coord-participation-independent-state` | qualified_participation_state | PASS 4/4 | PASS 4/4 |
| `coord-deadline-readiness` | obligation_and_readiness_gates | PASS 4/4 | PASS 4/4 |
| `coord-handoff-logistics` | planned_and_actual_handoff | PASS 4/4 | PASS 4/4 |
| `coord-arrangement-independent-state` | arrangement_state_separation | PASS 4/4 | PASS 4/4 |
| `coord-vacation-game-continuity` | cross_domain_checkpoint_continuity | PASS 4/4 | PASS 4/4 |
| `coord-weekly-brief-change-impact` | authority_aware_change_impact_brief | PASS 4/4 | PASS 4/4 |
| `coord-read-only-capability-boundary` | read_only_authorization_boundary | PASS 4/4 | PASS 4/4 |
| `coord-minor-export-policy` | fact_scoped_authority_and_redaction | PASS 4/4 | PASS 4/4 |

## Findings
- Highest complete-case rate: Filesystem agent, Native Straylight API agent at 100%.
- Native service calls averaged 2.5 per case and returned 19,288 characters in 724.0 ms of measured API time.
- Of that model-visible service output, 12,665 characters were evidence text and 6,623 were transport metadata; replay-weighted output was 46,195 characters per case.
- Native uncached model input was 1.31x the filesystem baseline.

## Interpretation boundary
This suite uses one model and deterministic concept-token groups with required citations and forbidden-conclusion checks. Exact-phrase false negatives were corrected through the recorded regrade path; the model outputs were not regenerated during regrading. The result tests source-faithful work products and checkpoint behavior, not every possible model-policy interaction.

## Conclusions
- The evaluated access surfaces covered people, identity reversal, roles, recurring events, logistics, readiness, vacation, game continuity, policy, and read-only authorization.
- The generic object, claim, qualified-relation, temporal, named-state, policy, and checkpoint kernel is sufficient for these work and personal coordination patterns without domain-specific runner logic.
- The native service scored 0.980 and persisted 14/14 eligible checkpoints. The durable and authorization behavior is product-relevant; small score differences are not superiority evidence by themselves.
- The native API improved complete-case and claim recall over filesystem access, but still used more uncached input on this compact suite. Compact projections and model tool policy remain optimization targets.
- The native condition exercised OpenAI embeddings, hybrid ranking, authority-aware retrieval, snapshot pinning, and capability-bound writes; the suite is not an isolated semantic hit-rate benchmark.
- The separate changed-evidence transition suite remains the decisive fresh-agent continuation and efficiency gate.

## Limitations
- The corpus is synthetic and the rubrics were authored with knowledge of it; untouched holdout tasks are still required.
- The fixed pack is a strong task-specific handoff control, not a generic retrieval baseline.
- The filesystem and workspace agents chose their own evidence paths, so token and latency differences include tool-policy behavior.
- Cumulative input includes cached-history replay. Uncached input is a better comparison of newly processed context, but it is not a direct measure of unique evidence or cost.
- Concept-token grading is deterministic and preserves explicit negation, identifiers, citations, and forbidden conclusions, but it is not a complete semantic judge. The final regrade was manually audited at the claim level.
- The filesystem condition was instruction-restricted rather than OS-sandboxed.
- The native condition is the containerized Rust, Postgres/pgvector, MinIO, and OpenAI implementation; Python and SQLite remain evaluation controls only.
- Read-only denial is executable in this suite; the separate destructive live smoke covers cross-user isolation and every native mutation surface.
- Live telemetry, external websites, and changing production state were intentionally unavailable.

## Reproduce
```bash
cd /Users/Shared/projects/straylight
python3 -m unittest discover -s tests -v
python3 agent_work_eval.py --manifest eval/personal_coordination_cases.json validate
python3 agent_work_eval.py --manifest eval/personal_coordination_cases.json run --filesystem-native --concurrency 3 --timeout 420 --out results/native-personal-coordination.json
```

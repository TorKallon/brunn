Created: 2026-07-26T21:33:16-07:00
Updated: 2026-07-26T21:33:16-07:00
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
| Filesystem agent | 10/15 (67%) | 52/60 (87%) | 0.965 | output only |
| Native Straylight API agent | 8/15 (53%) | 50/60 (83%) | 0.964 | 14/14 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Filesystem agent | 81,837 | 66,594 | 15,243 | 1,938 | 3.0 | 17,892 |
| Native Straylight API agent | 92,232 | 75,725 | 16,508 | 2,207 | 3.2 | 16,431 |

## Workload results
| Workload | Filesystem agent | Native Straylight API agent |
| --- | ---: | ---: |
| Personal Coordination | 10/15 cases, 52/60 claims | 8/15 cases, 50/60 claims |

## Per-case results
| Case | Capability | Filesystem agent | Native Straylight API agent |
| --- | --- | ---: | ---: |
| `coord-person-resolution` | identity_resolution_with_provenance | FAIL 3/4 | PASS 4/4 |
| `coord-identity-equivalence-reversal` | reversible_identity_equivalence | PASS 4/4 | PASS 4/4 |
| `coord-person-dossier` | derived_person_view | PASS 4/4 | PASS 4/4 |
| `coord-role-relationship-provenance` | qualified_role_relation_and_authority | FAIL 3/4 | FAIL 3/4 |
| `coord-canonical-contract-normalization` | canonical_contract_normalization | FAIL 3/4 | FAIL 3/4 |
| `coord-series-exceptions` | recurring_event_identity_and_exceptions | PASS 4/4 | PASS 4/4 |
| `coord-schedule-supersession` | authoritative_temporal_supersession | PASS 4/4 | PASS 4/4 |
| `coord-participation-independent-state` | qualified_participation_state | PASS 4/4 | PASS 4/4 |
| `coord-deadline-readiness` | obligation_and_readiness_gates | PASS 4/4 | FAIL 2/4 |
| `coord-handoff-logistics` | planned_and_actual_handoff | PASS 4/4 | PASS 4/4 |
| `coord-arrangement-independent-state` | arrangement_state_separation | PASS 4/4 | FAIL 3/4 |
| `coord-vacation-game-continuity` | cross_domain_checkpoint_continuity | PASS 4/4 | FAIL 3/4 |
| `coord-weekly-brief-change-impact` | authority_aware_change_impact_brief | FAIL 1/4 | FAIL 2/4 |
| `coord-read-only-capability-boundary` | read_only_authorization_boundary | PASS 4/4 | PASS 4/4 |
| `coord-minor-export-policy` | fact_scoped_authority_and_redaction | FAIL 2/4 | FAIL 2/4 |

## Findings
- Highest complete-case rate: Filesystem agent at 67%.
- Native service calls averaged 2.2 per case and returned 15,487 characters in 161.3 ms of measured API time.
- Of that model-visible service output, 8,764 characters were evidence text and 6,723 were transport metadata; replay-weighted output was 31,541 characters per case.
- Native uncached model input was 1.08x the filesystem baseline.

## Interpretation boundary
This suite uses one model and deterministic concept-token groups with required citations and forbidden-conclusion checks. Exact-phrase false negatives were corrected through the recorded regrade path; the model outputs were not regenerated during regrading. The result tests source-faithful work products and checkpoint behavior, not every possible model-policy interaction.

## Conclusions
- The evaluated access surfaces covered people, identity reversal, roles, recurring events, logistics, readiness, vacation, game continuity, policy, and read-only authorization.
- The generic object, claim, qualified-relation, temporal, named-state, policy, and checkpoint kernel is sufficient for these work and personal coordination patterns without domain-specific runner logic.
- The native service scored 0.964 and persisted 14/14 eligible checkpoints. The durable and authorization behavior is product-relevant; small score differences are not superiority evidence by themselves.
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

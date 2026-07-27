Created: 2026-07-27T10:18:37-07:00
Updated: 2026-07-27T10:18:37-07:00
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

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Filesystem agent | 5/15 (33%) | 45/60 (75%) | 0.934 | output only |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Filesystem agent | 72,746 | 53,584 | 19,162 | 1,670 | 3.3 | 15,178 |

## Workload results
| Workload | Filesystem agent |
| --- | ---: |
| Personal Coordination | 5/15 cases, 45/60 claims |

## Per-case results
| Case | Capability | Filesystem agent |
| --- | --- | ---: |
| `coord-person-resolution` | identity_resolution_with_provenance | FAIL 3/4 |
| `coord-identity-equivalence-reversal` | reversible_identity_equivalence | PASS 4/4 |
| `coord-person-dossier` | derived_person_view | FAIL 3/4 |
| `coord-role-relationship-provenance` | qualified_role_relation_and_authority | FAIL 3/4 |
| `coord-canonical-contract-normalization` | canonical_contract_normalization | FAIL 3/4 |
| `coord-series-exceptions` | recurring_event_identity_and_exceptions | PASS 4/4 |
| `coord-schedule-supersession` | authoritative_temporal_supersession | FAIL 3/4 |
| `coord-participation-independent-state` | qualified_participation_state | FAIL 3/4 |
| `coord-deadline-readiness` | obligation_and_readiness_gates | FAIL 1/4 |
| `coord-handoff-logistics` | planned_and_actual_handoff | PASS 4/4 |
| `coord-arrangement-independent-state` | arrangement_state_separation | FAIL 3/4 |
| `coord-vacation-game-continuity` | cross_domain_checkpoint_continuity | PASS 4/4 |
| `coord-weekly-brief-change-impact` | authority_aware_change_impact_brief | FAIL 1/4 |
| `coord-read-only-capability-boundary` | read_only_authorization_boundary | PASS 4/4 |
| `coord-minor-export-policy` | fact_scoped_authority_and_redaction | FAIL 2/4 |

## Findings
- Highest complete-case rate: Filesystem agent at 33%.

## Interpretation boundary
This suite uses one model and deterministic concept-token groups with required citations and forbidden-conclusion checks. Exact-phrase false negatives were corrected through the recorded regrade path; the model outputs were not regenerated during regrading. The result tests source-faithful work products and checkpoint behavior, not every possible model-policy interaction.

## Conclusions
- The evaluated access surfaces covered people, identity reversal, roles, recurring events, logistics, readiness, vacation, game continuity, policy, and read-only authorization.
- The generic object, claim, qualified-relation, temporal, named-state, policy, and checkpoint kernel is sufficient for these work and personal coordination patterns without domain-specific runner logic.
- The workspace scored 0.934 and persisted 0/0 eligible checkpoints. The durable and authorization behavior is product-relevant; small score differences are not superiority evidence by themselves.
- The shell prototype is too interactive: it used substantially more calls, cumulative input, uncached input, and returned text than direct filesystem access. The typed native API must preserve quality while collapsing those round trips.
- Lexical BM25 was sufficient on this compact synthetic corpus. OpenAI embeddings, hybrid ranking, authority-aware traversal, hit rate, and search latency remain untested target-architecture hypotheses.
- The next implementation gate is the Rust/Postgres service and native typed adapter, followed by untouched holdout tasks and the existing changed-evidence checkpoint-transition suite.

## Limitations
- The corpus is synthetic and the rubrics were authored with knowledge of it; untouched holdout tasks are still required.
- The fixed pack is a strong task-specific handoff control, not a generic retrieval baseline.
- The filesystem and workspace agents chose their own evidence paths, so token and latency differences include tool-policy behavior.
- Cumulative input includes cached-history replay. Uncached input is a better comparison of newly processed context, but it is not a direct measure of unique evidence or cost.
- Concept-token grading is deterministic and preserves explicit negation, identifiers, citations, and forbidden conclusions, but it is not a complete semantic judge. The final regrade was manually audited at the claim level.
- The filesystem condition was instruction-restricted rather than OS-sandboxed.
- The workspace condition is a BM25-backed Python shell prototype, not the planned Rust, Postgres, OpenAI-embedding, and MinIO architecture.
- Read-only denial is executable across the prototype command surface; the production service still needs cross-user and capability tests for every native API operation.
- Live telemetry, external websites, and changing production state were intentionally unavailable.

## Reproduce
```bash
cd /Users/Shared/projects/straylight
python3 -m unittest discover -s tests -v
python3 agent_work_eval.py --manifest eval/personal_coordination_cases.json validate
python3 agent_work_eval.py --manifest eval/personal_coordination_cases.json run --concurrency 3 --timeout 420 --out results/native-personal-coordination.json
```

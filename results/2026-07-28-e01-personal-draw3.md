Created: 2026-07-28T04:04:53-07:00
Updated: 2026-07-28T04:04:53-07:00
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
- Service build revision: `232e7c6c8b8675d3ceeee3ff02a600ffdb95d2da`
- Expected runtime features: `{"intention_ledger":false,"lexical_single_scan":false,"read_path_roundtrip_v1":false,"resume_deltas":false,"search_char_cap":false,"search_fair_share":false,"search_section_demotion_top_n":null,"search_top1_hydration":false,"semantic_lane":false,"supersession_demotion":false,"verbatim_spans":false}`
- Actual runtime features: `{"allow_degraded_embeddings":true,"embed_cache":true,"embedding_backfill_batch_chunks":64,"embedding_backfill_foreground_status_timeout_ms":1000,"embedding_backfill_foreground_status_url_configured":true,"embedding_backfill_guard":true,"embedding_backfill_inter_batch_ms":250,"embedding_backfill_open_p95_limit_ms":120.0,"embedding_backfill_search_p95_limit_ms":107.0,"intention_ledger":false,"lexical_single_scan":false,"materialize_token_budget":24000,"observability_timings_ms":true,"read_path_roundtrip_v1":false,"resume_deltas":false,"search_char_cap":false,"search_fair_share":false,"search_section_demotion_top_n":null,"search_top1_hydration":false,"semantic_deadline_ms":300,"semantic_lane":false,"supersession_demotion":false,"supersession_demotion_weight":1.5,"verbatim_spans":false}`
- Embedding posture: `{"dimensions":1536,"model":"straylight-hashing-v1","provider":"hashing","status":"degraded"}`

## Conditions
- **Native Straylight API agent:** a fresh agent receives no corpus path and uses the Rust service through batched `open`, `query`, `read`, `compute`, `verify`, and capability-bound write operations. Read-only credentials cannot persist a checkpoint.
- **Filesystem agent:** a fresh agent receives the frozen corpus and ordinary read/search/script tools.
- **Filesystem agent with writable sidecar:** the corpus remains read-only, while a run-scoped sidecar must receive a durable JSON checkpoint.

## Results
| Condition | Cases passed | Claims passed | Mean score | Persisted checkpoints |
| --- | ---: | ---: | ---: | ---: |
| Native Straylight API agent | 4/15 (27%) | 46/60 (77%) | 0.949 | 14/14 eligible |
| Filesystem agent | 7/15 (47%) | 49/60 (82%) | 0.953 | output only |
| Filesystem agent with writable sidecar | 10/15 (67%) | 52/60 (87%) | 0.966 | 15/15 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Native Straylight API agent | 86,404 | 68,437 | 17,967 | 2,134 | 3.2 | 12,666 |
| Filesystem agent | 77,755 | 63,539 | 14,215 | 1,956 | 3.1 | 16,644 |
| Filesystem agent with writable sidecar | 137,650 | 118,272 | 19,378 | 2,818 | 4.9 | 14,370 |

## Workload results
| Workload | Native Straylight API agent | Filesystem agent | Filesystem agent with writable sidecar |
| --- | ---: | ---: | ---: |
| Personal Coordination | 4/15 cases, 46/60 claims | 7/15 cases, 49/60 claims | 10/15 cases, 52/60 claims |

## Per-case results
| Case | Capability | Native Straylight API agent | Filesystem agent | Filesystem agent with writable sidecar |
| --- | --- | ---: | ---: | ---: |
| `coord-person-resolution` | identity_resolution_with_provenance | FAIL 3/4 | FAIL 3/4 | PASS 4/4 |
| `coord-identity-equivalence-reversal` | reversible_identity_equivalence | FAIL 3/4 | PASS 4/4 | PASS 4/4 |
| `coord-person-dossier` | derived_person_view | FAIL 3/4 | PASS 4/4 | PASS 4/4 |
| `coord-role-relationship-provenance` | qualified_role_relation_and_authority | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `coord-canonical-contract-normalization` | canonical_contract_normalization | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `coord-series-exceptions` | recurring_event_identity_and_exceptions | FAIL 3/4 | PASS 4/4 | PASS 4/4 |
| `coord-schedule-supersession` | authoritative_temporal_supersession | PASS 4/4 | FAIL 3/4 | PASS 4/4 |
| `coord-participation-independent-state` | qualified_participation_state | FAIL 2/4 | PASS 4/4 | PASS 4/4 |
| `coord-deadline-readiness` | obligation_and_readiness_gates | FAIL 2/4 | FAIL 2/4 | FAIL 2/4 |
| `coord-handoff-logistics` | planned_and_actual_handoff | FAIL 3/4 | PASS 4/4 | PASS 4/4 |
| `coord-arrangement-independent-state` | arrangement_state_separation | PASS 4/4 | FAIL 3/4 | PASS 4/4 |
| `coord-vacation-game-continuity` | cross_domain_checkpoint_continuity | FAIL 3/4 | PASS 4/4 | PASS 4/4 |
| `coord-weekly-brief-change-impact` | authority_aware_change_impact_brief | FAIL 2/4 | FAIL 1/4 | FAIL 1/4 |
| `coord-read-only-capability-boundary` | read_only_authorization_boundary | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `coord-minor-export-policy` | fact_scoped_authority_and_redaction | PASS 4/4 | FAIL 3/4 | FAIL 3/4 |

## Findings
- Highest complete-case rate: Filesystem agent with writable sidecar at 67%.
- Native service calls averaged 2.7 per case and returned 12,165 characters in 156.8 ms of measured API time.
- Of that model-visible service output, 5,990 characters were evidence text and 6,175 were transport metadata; replay-weighted output was 29,386 characters per case.
- Native uncached model input was 1.26x the filesystem baseline.

## Interpretation boundary
This suite uses one model and deterministic concept-token groups with required citations and forbidden-conclusion checks. Exact-phrase false negatives were corrected through the recorded regrade path; the model outputs were not regenerated during regrading. The result tests source-faithful work products and checkpoint behavior, not every possible model-policy interaction.

## Conclusions
- The evaluated access surfaces covered people, identity reversal, roles, recurring events, logistics, readiness, vacation, game continuity, policy, and read-only authorization.
- The generic object, claim, qualified-relation, temporal, named-state, policy, and checkpoint kernel is sufficient for these work and personal coordination patterns without domain-specific runner logic.
- The native service scored 0.949 and persisted 14/14 eligible checkpoints. The durable and authorization behavior is product-relevant; small score differences are not superiority evidence by themselves.
- The native API improved complete-case and claim recall over filesystem access, but still used more uncached input on this compact suite. Compact projections and model tool policy remain optimization targets.
- The native condition exercised exact and lexical retrieval, authority-aware retrieval, snapshot pinning, and capability-bound writes. Semantic retrieval and OpenAI embeddings were disabled for E01.
- The separate changed-evidence transition suite remains the decisive fresh-agent continuation and efficiency gate.

## Limitations
- The corpus is synthetic and the rubrics were authored with knowledge of it; untouched holdout tasks are still required.
- The fixed pack is a strong task-specific handoff control, not a generic retrieval baseline.
- The filesystem and workspace agents chose their own evidence paths, so token and latency differences include tool-policy behavior.
- Cumulative input includes cached-history replay. Uncached input is a better comparison of newly processed context, but it is not a direct measure of unique evidence or cost.
- Concept-token grading is deterministic and preserves explicit negation, identifiers, citations, and forbidden conclusions, but it is not a complete semantic judge. E01 retained the original grading and did not run a regrade.
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

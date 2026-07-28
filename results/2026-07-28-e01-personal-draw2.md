Created: 2026-07-28T03:28:57-07:00
Updated: 2026-07-28T03:28:57-07:00
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
| Native Straylight API agent | 7/15 (47%) | 49/60 (82%) | 0.950 | 14/14 eligible |
| Filesystem agent | 6/15 (40%) | 47/60 (78%) | 0.954 | output only |
| Filesystem agent with writable sidecar | 7/15 (47%) | 49/60 (82%) | 0.962 | 15/15 eligible |

## Token and tool accounting
`input_tokens` is cumulative across the complete multi-call agent turn. Cached conversation history is counted again when a later tool result triggers another model call, so it is not a measure of unique evidence loaded.

| Condition | Cumulative input | Cached input | Uncached input | Output | Completed tool calls | Recorded tool output chars |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Native Straylight API agent | 94,205 | 72,977 | 21,228 | 2,322 | 3.5 | 13,000 |
| Filesystem agent | 76,744 | 56,422 | 20,322 | 1,771 | 3.0 | 14,249 |
| Filesystem agent with writable sidecar | 135,514 | 111,019 | 24,495 | 2,830 | 4.7 | 14,748 |

## Workload results
| Workload | Native Straylight API agent | Filesystem agent | Filesystem agent with writable sidecar |
| --- | ---: | ---: | ---: |
| Personal Coordination | 7/15 cases, 49/60 claims | 6/15 cases, 47/60 claims | 7/15 cases, 49/60 claims |

## Per-case results
| Case | Capability | Native Straylight API agent | Filesystem agent | Filesystem agent with writable sidecar |
| --- | --- | ---: | ---: | ---: |
| `coord-person-resolution` | identity_resolution_with_provenance | FAIL 3/4 | PASS 4/4 | PASS 4/4 |
| `coord-identity-equivalence-reversal` | reversible_identity_equivalence | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `coord-person-dossier` | derived_person_view | FAIL 3/4 | FAIL 3/4 | PASS 4/4 |
| `coord-role-relationship-provenance` | qualified_role_relation_and_authority | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `coord-canonical-contract-normalization` | canonical_contract_normalization | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `coord-series-exceptions` | recurring_event_identity_and_exceptions | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `coord-schedule-supersession` | authoritative_temporal_supersession | PASS 4/4 | FAIL 3/4 | PASS 4/4 |
| `coord-participation-independent-state` | qualified_participation_state | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `coord-deadline-readiness` | obligation_and_readiness_gates | FAIL 2/4 | FAIL 1/4 | FAIL 2/4 |
| `coord-handoff-logistics` | planned_and_actual_handoff | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `coord-arrangement-independent-state` | arrangement_state_separation | FAIL 3/4 | FAIL 3/4 | FAIL 3/4 |
| `coord-vacation-game-continuity` | cross_domain_checkpoint_continuity | PASS 4/4 | PASS 4/4 | FAIL 3/4 |
| `coord-weekly-brief-change-impact` | authority_aware_change_impact_brief | FAIL 1/4 | FAIL 1/4 | FAIL 1/4 |
| `coord-read-only-capability-boundary` | read_only_authorization_boundary | PASS 4/4 | PASS 4/4 | PASS 4/4 |
| `coord-minor-export-policy` | fact_scoped_authority_and_redaction | PASS 4/4 | FAIL 3/4 | FAIL 3/4 |

## Findings
- Highest complete-case rate: Native Straylight API agent, Filesystem agent with writable sidecar at 47%.
- Native service calls averaged 2.7 per case and returned 12,280 characters in 124.9 ms of measured API time.
- Of that model-visible service output, 5,656 characters were evidence text and 6,624 were transport metadata; replay-weighted output was 30,343 characters per case.
- Native uncached model input was 1.04x the filesystem baseline.

## Interpretation boundary
This suite uses one model and deterministic concept-token groups with required citations and forbidden-conclusion checks. Exact-phrase false negatives were corrected through the recorded regrade path; the model outputs were not regenerated during regrading. The result tests source-faithful work products and checkpoint behavior, not every possible model-policy interaction.

## Conclusions
- The evaluated access surfaces covered people, identity reversal, roles, recurring events, logistics, readiness, vacation, game continuity, policy, and read-only authorization.
- The generic object, claim, qualified-relation, temporal, named-state, policy, and checkpoint kernel is sufficient for these work and personal coordination patterns without domain-specific runner logic.
- The native service scored 0.950 and persisted 14/14 eligible checkpoints. The durable and authorization behavior is product-relevant; small score differences are not superiority evidence by themselves.
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

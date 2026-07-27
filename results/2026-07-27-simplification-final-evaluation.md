# Straylight simplification final evaluation

Created: 2026-07-27

## Decision

The simplification should not be rolled back.

It removes the production-blocking latency and consistency behavior by large,
repeatable margins. Reasoning quality is broadly preserved, but the evidence is
not strong enough to claim perfect parity:

- one full strict draw scored the simplified version 10 claims below the legacy
  version across 228 claims;
- a targeted matched repeat over every ordinary case where legacy initially won
  reduced that difference to one claim across 64 claims;
- the simplified service had a rubric-accepted source in its returned context
  for 21 of the 22 legacy-only claim wins;
- all five changed-evidence continuations preserved exact parent, revision,
  prior-source, delta-source, and checkpoint lineage;
- RuptureOps remains the only consistent warning signal: the simplified version
  returned substantially more context and was two claims behind legacy in the
  targeted repeat.

The defensible conclusion is therefore: no material retrieval-driven reasoning
degradation has been demonstrated, but a small context-overload risk remains.
That risk is narrow enough to optimize on the simplified architecture rather
than retain an architecture that times out.

## Compared versions

- Performance-impacted API image: commit `dd0275678821677bd0bdaaf28bd3cecbe1687e96`,
  image `sha256:8098fdce955f5c3b3f1b008d54cf183a5283e7a34b5e302c129dd6016b79bdb3`.
- Simplified API image: commit `c3a54200f0dbb0cb02d0077730995119178cb31e`,
  image `sha256:5892103ec5afea553b835dff969a6b2a60b60ffbbcfc0e1ce685f447da9fb228`.
- Reasoning model: `gpt-5.6-sol`, with the same API-authenticated harness,
  source corpora, prompts, answer schema, and strict rubric for each condition.
- Performance driver: the same `performance_eval.py`, deterministic corpus,
  unknown-path discovery task, query limits, mock embedding provider, and clean
  disposable PostgreSQL databases.

The owner deployment and owner data were not used or modified.

## Full reasoning evaluation

All 171 runs completed successfully: 57 each for legacy Straylight, simplified
Straylight, and direct Markdown. There were no model, service, or harness
timeouts.

| Suite | Claims | Legacy | Simplified | Direct Markdown |
| --- | ---: | ---: | ---: | ---: |
| Established project work | 52 | 42 | 41 | 45 |
| Recent Europe and Aether work | 48 | 35 | 33 | 35 |
| RuptureOps and StarRupture | 48 | 39 | 33 | 35 |
| Personal coordination | 60 | 47 | 44 | 45 |
| Changed-evidence continuations | 20 | 7 | 9 | 11 |
| **Total** | **228** | **170 (74.6%)** | **160 (70.2%)** | **171 (75.0%)** |

The raw simplified-to-legacy gap is -10 claims, or -4.39 percentage points.
The raw simplified-to-files gap is -11 claims, or -4.82 points. A single model
draw cannot establish whether that difference is architectural, so every
ordinary case in which legacy beat simplified was repeated against both APIs.

## Targeted matched repeat

The repeat covered 16 cases and 64 claims selected only because legacy won the
first draw.

| Suite | Claims | Legacy repeat | Simplified repeat |
| --- | ---: | ---: | ---: |
| Established project work | 12 | 9 | 8 |
| Recent Europe and Aether work | 12 | 8 | 8 |
| RuptureOps and StarRupture | 24 | 20 | 18 |
| Personal coordination | 16 | 9 | 11 |
| **Total** | **64** | **46 (71.9%)** | **45 (70.3%)** |

The repeated gap is one claim, or 1.56 percentage points. Simplified wins the
personal subset, ties recent work, trails project work by one, and trails
RuptureOps by two.

## Source and lineage audit

The first draw contained 22 claims passed by legacy and failed by simplified.
The audit found:

- 20 simplified answers directly cited a rubric-accepted source;
- 21 simplified service responses contained a rubric-accepted source;
- the remaining response contained the correct four save-state facts from
  equivalent normalized model records, but not the exact path accepted by the
  path-based grader;
- another failed claim used "safe cache outside the contested base failure
  domain," which is semantically equivalent to the rubric's "safe recovery
  cache";
- one public/private artifact answer was substantively overcautious even though
  it identified the separately licensed data path. This is a real answer-quality
  miss, not a missing-source failure.

All 20 continuation claims had their expected source available. All five
simplified continuation runs also passed durable structural checks:

- child checkpoint committed and read back through HTTP;
- exact parent checkpoint matched;
- expected revision matched;
- prior and changed-evidence sources were both preserved;
- service call budget was respected;
- no provenance or lineage errors occurred.

The continuation claim misses were answer omissions or strict phrase misses,
not failed checkpoint recovery.

## Token use

Mean uncached model input across all 57 cases:

| Condition | Mean uncached input |
| --- | ---: |
| Legacy Straylight | 24,092 tokens |
| Simplified Straylight | 24,260 tokens |
| Direct Markdown | 25,168 tokens |

Simplified is effectively tied with legacy (+0.7%) and uses 3.6% fewer uncached
tokens than direct files. The aggregate hides uneven context selection:
RuptureOps repeat responses averaged 70,814 service characters for simplified
versus 41,441 for legacy, with 29,752 versus 24,583 uncached tokens. That
overfetch is the leading explanation for the residual RuptureOps signal.

## Exact performance reproductions

Times below are p95 unless identified as a first sample. The fresh paired runs
used the exact same generated records, corpus bytes, task, query, limits, and
driver against clean old and new stacks.

| Scenario | Legacy | Simplified | Result |
| --- | ---: | ---: | --- |
| Retained 500-record open | 39.762 s first, 35.833 s p95 | n/a | Preserved proof of the reported 30-second class behavior |
| Fresh paired 500 import | 11.192 s | 0.872 s | 12.8x faster |
| Fresh paired 500 open | 18.217 s | 0.183 s | 99.4x faster |
| Fresh paired 500 search | 8.682 s | 0.081 s | 107.1x faster |
| Cold paired 1,500 import | 89.019 s | 3.557 s | 25.0x faster |
| Cold paired 1,500 open | 10.449 s | 0.651 s | 16.0x faster |
| Cold paired 1,500 search | 10.436 s | 0.704 s | 14.8x faster |
| Accumulated 3,340 run | HTTP 408 after 26.088 s | Full pass | Legacy fails before useful retrieval |
| Simplified 3,340 open | n/a after legacy failure | 1.047 s | Target found in all samples |
| Simplified 3,340 search | n/a after legacy failure | 0.674 s | Target found in all samples |
| Simplified 3,340 broad search | n/a after legacy failure | 1.867 s | Sources found in all samples |
| Direct-file 3,340 discovery | 0.124 s | 0.119 s control | Same deterministic file corpus |

The legacy 3,340 request spent 25.042 seconds in
`straylight_auth.read_manifest_stats(...)` and then returned
`database_timeout` at 26.088 seconds. It failed before the harness could collect
an open or search sample. The simplified version imported the same 3,340
records in 8.114 seconds, immediately retrieved while semantic indexing was
still pending, completed all open/search/read/checkpoint/resume probes, and
passed every gate.

The simplification also removed work that did not help reasoning:

| Measure | Legacy 500 | Simplified 500 | Change |
| --- | ---: | ---: | ---: |
| Response payload | 589,677 chars | 127,155 chars | -78.4% |
| Rows added by one checkpoint | 2,520 | 11 | -99.6% |
| Protocol-to-evidence ratio | 2.377 | 0.393 | -83.5% |

At 1,500 records, payload fell from 972,715 to 217,036 characters and one
checkpoint fell from 7,520 rows to 11.

## Verification

- Python suite: 237 passed, 5 explicitly skipped, 0 failed.
- Rust suite: 234 unit tests and 2 integration tests passed, 1 ignored, 0 failed.
- Work, recent-work, RuptureOps, personal, and transition manifests validated.
- Every paired performance target was found when the request completed.
- The simplified 500, 1,500, and 3,340 performance runs passed their retrieval,
  latency, semantic-pending, protocol-overhead, concurrent-write, and bounded
  checkpoint-growth gates.

## Final verdict

The old version is not a viable fallback. It can spend tens of seconds proving
global manifest state before returning useful context, emits several times more
protocol than evidence, and copies thousands of rows into a checkpoint.

The simplified version restores practical behavior without evidence of lost
records, lost continuation state, or a material token increase. Its strict
single-draw reasoning score was lower, so the quality result is not a clean
win. The matched repeat and source audit reduce the plausible architecture
regression to a narrow RuptureOps/context-budget concern.

Proceed with the simplified architecture. Keep the full reasoning suite as a
release gate and make RuptureOps result budgeting/ranking the next quality
experiment. Optimize by returning less already-available context, not by adding
schema or restoring synchronous consistency work.

## Evidence

Reasoning:

- `results/2026-07-27-final-api-{old,new,files}-{work,recent,rupture,personal,transitions}.json`
- `results/2026-07-27-tiebreak-{old,new}-{work,recent,rupture,personal}.json`

Performance:

- `results/2026-07-26-dd02756-performance.json`
- `results/2026-07-27-dd02756-exact-timeout-reproduction.json`
- `results/2026-07-27-c3a5420-exact-timeout-reproduction.json`
- `results/2026-07-27-dd02756-cold-1500-timeout.json`
- `results/2026-07-27-c3a5420-cold-1500-timeout.json`
- `results/2026-07-27-dd02756-accumulated-3340-timeout.json`
- `results/2026-07-27-dd02756-accumulated-3340-api-log.txt`
- `results/2026-07-27-c3a5420-accumulated-3340-timeout.json`

Harness:

- `agent_work_eval.py`
- `transition_eval.py`
- `performance_eval.py`
- `eval/work_cases.json`
- `eval/recent_work_cases.json`
- `eval/rupture_ops_cases.json`
- `eval/personal_coordination_cases.json`
- `eval/transition_cases.json`

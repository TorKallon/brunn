# D02 — Verbatim Span Contract

Status: Implemented behind default-off flag — awaiting E02 stage 2
Date: 2026-07-27
Depends on: none
Gated by: E02 (E02-verbatim-identifier-gate.md)
Runtime flag: verbatim_spans

## Problem and evidence

The simplified core loses literal values that plain files preserve.

1. Losses are context compilation, not retrieval: in the 57-case strict draw, 21/22 disputed simplified answers had a rubric-accepted source already present in the returned context.
2. The interface run on the simplified core scored native API 186/228 vs files 194/228. Files hand the model raw text; the API hands it compiled excerpts.
3. The excerpt pipeline truncates: 2,400 chars/excerpt, ≤3 sections/entry, 96,000 chars/response, 128 candidates. An exact-lane hit (flat score 10.0) guarantees the entry surfaces, but not that the literal matching line survives section selection. An identifier at char 5,000 of a matched entry can be excerpted away, leaving the model to paraphrase — the documented paraphrase/exact-value loss family.
4. Chronic identifier-heavy failures sit exactly here: recent-aether-gmail-actions, recent-europe-calendar-dedup, recent-aether-morning-brief, and the tracker cases (message ids, event ids, literal field values).
5. Headroom exists: search p95 53.1ms at the 640K soak (results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json), against a 3,000ms hard gate.

## Design

With verbatim_spans on, every exact-lane candidate in a search response additionally carries:

`verbatim_matches`: up to 3 objects `{line_no, byte_start, byte_end, text, version, content_hash}`.

- `text` is the full literal source line containing the exact-phrase match — untrimmed, never re-wrapped, never elided. Selection is the first ≤3 matching lines in document order.
- Exempt from the 2,400-char excerpt truncation: the line is returned verbatim even when it lies outside every selected excerpt window. A per-line hard cap of 2,400 chars applies only in the pathological single-huge-line case and sets `truncated: true`.
- `content_hash` is the sha256 of the full source of the matched entry version — the same hash used in checkpoint source refs ("path | version N | sha256:..."). `byte_start`/`byte_end` are offsets into that hashed byte stream. Each citation is therefore mechanically verifiable and joins directly to checkpoint lineage. A plain file tree cannot provide hash-anchored, offset-stable citations; this is capability files categorically lack.
- Zero-bloat bounds: 3 lines per candidate; aggregate verbatim payload ≤9,600 chars/response, accounted separately from (not charged against) the 96,000-char excerpt budget. OWNER DECISION: the 9,600-char aggregate cap is provisional; E02 stage 2 payload measurements may justify lowering it.
- Computation happens at response-assembly time against source text the exact lane already hydrated. Zero additional SQL round trips; a per-operation query-count assertion ships with the change (hard constraint: round-trip budgets accompany latency gates).
- MCP preservation: `memory.query` in apps/mcp passes `verbatim_matches` through byte-for-byte into its single text-content-block JSON. The reasoning view must never summarize, dedup, or re-wrap these fields; a contract test asserts payload equality end to end.
- Ranking untouched: spans annotate candidates; they never contribute to scoring or ordering.

Request delta: none (server-side flag). Response delta: the single field above on exact-lane candidates only. Lexical and semantic lanes are out of scope for this design.

Implementation note (2026-07-27): `STRAYLIGHT_VERBATIM_SPANS=false` is
plumbed through startup configuration and Compose. When enabled, an exact path
query conditionally retrieves that version's full source in the same SQL
round trip, extracts at most three query-anchor lines, and returns
hash/version/byte-pinned matches under the 9,600-character response cap.
Flag-off SQL still selects only the existing 2,400-character prefix. The MCP
reasoning-view compactor preserves `verbatim_matches` structurally unchanged.
API and MCP contract tests cover a planted identifier beyond byte 2,400,
offset-preserving truncation, and passthrough. E02's deterministic and
reasoning gates remain unrun, so the flag remains off.

## What this does NOT change

- No schema expansion: spans are computed from entry_versions source at read time; no new tables, columns, or indexes.
- Scoring and lane structure: exact 10.0 flat, lexical 3.0+ts_rank_cd, semantic 2.0+(1−distance), and the recent-first two-tier lexical window are untouched.
- All other budgets: 128 candidates, 96,000/2,400/≤3-section excerpt caps, ≤16 batched queries.
- Markdown authority: nothing durable is authored; spans are derived and reproduce identically after rebuild-from-vault.
- Write path, checkpoint format (11 rows/~55KB), open path, semantic-off critical path.

## Failure-mode analysis

- Dedup revert (2026-07-22 — context reduction hurt quality, reverted): D02 removes nothing; it is strictly additive under a hard cap. It is not in the guilty-until-proven reduction class, but it is still gated by an n≥3 paired experiment per hard constraint.
- v6 recent-first collapse (recency ranking hid older authoritative sources, Star Rupture 0/3): no ranking, windowing, or recency change of any kind.
- 2026-07-26 production collapse (unbudgeted synchronous bookkeeping): span extraction is per-response, in-process, on already-hydrated bytes; zero extra queries, asserted in tests; covered by the search p95 gate at the 640K soak, which is the only gate that caught both prior write-path regressions (v5 3,404ms, v7 3,170ms).
- Overfetch (RuptureOps ~70,814 vs legacy ~41,441 service chars/case — leading quality risk): worst-case growth is capped at 3 lines/candidate and 9,600 chars/response; E02 measures the realized per-case delta, which for typical identifier lines is tens of chars.
- Paraphrase/exact-value loss: the target. Hash-anchored literal lines turn "quote the source" into a copy operation instead of a generation.

## Acceptance gates

1. E02 stage 1 (Phase 0, pre-code) has documented the current defect: planted identifiers past char 2,400 absent verbatim from search payloads on the current build.
2. E02 stage 2 deterministic gate: 30/30 planted identifiers returned verbatim in-payload at 1k/10k/64k and the 640k soak with the flag on; both stages become permanent performance_eval gates.
3. E02 stage 2 reasoning gate: n≥3 paired service_api draws, exact McNemar; no overall regression; net improvement on identifier-heavy recent-work cases. Single-draw deltas are noise (±3-5 claims) and never load-bearing.
4. Search p95 within the ≤3,000ms hard gate and no drift beyond run noise vs the 53.1ms v8 baseline; query-count assertion holds (zero added round trips).
5. MCP byte-for-byte passthrough contract test green.

## Rollout and kill switch

Flag verbatim_spans, default off. Sequence: eval environment for E02 stage 2 → Nyx canaries under the Tier A/B plan (D14 frame) → default on only after all gates pass. Kill switch is the flag itself: runtime flip, no deploy, per hard constraint. Any gate failure leaves the flag off and returns D02 to design.

## References

- E02-verbatim-identifier-gate.md — gating experiment, both stages.
- results/2026-07-27-simplified-release-candidate-v8-future-soak-performance.json — search p95 53.1ms, open 59.7ms at 640K.
- 57-case strict draw record — 21/22 disputed answers had an accepted source in context; interface run native 186/228 vs files 194/228.
- apps/api/src/simple_core.rs — search assembly and caps; apps/mcp — memory.query reasoning view.
- Documented negative results: 2026-07-22 dedup revert; v6 recent-first collapse; 2026-07-26 bookkeeping collapse.

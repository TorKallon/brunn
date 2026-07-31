# E12 — E01 loss autopsy

Status: Specified — not run
Date: 2026-07-28
Gates: Tier B entry triage (D14, as amended 2026-07-28); any future context-shaping design doc
Phase: 0 (analysis-only; zero model-API spend; no product code changes)

## Question

Where do the service's E01 losses actually come from? E01 closed with the
service ~2–5 claims behind out of 236 (point estimate −4.667 vs sidecar, CI
[−13.667, +4.333]) while *disproving* the overfetch theory (service emitted
32,067 chars/case vs files' 101,406). The residual deficit therefore has no
confirmed mechanism. The most productive artifact in this project's history —
the 2026-07-22 flat-file loss analysis — was exactly this kind of qualitative
paired autopsy. E01 produced 531 saved case-runs nobody has read case-by-case.

## Preconditions and inputs

- The 15 immutable E01 draw JSONs and `results/2026-07-28-e01-aggregate.json`
  (SHA-256 ledger in `results/2026-07-28-e01-aggregate.md`). Read-only.
- `agent_work_eval.py regrade` / saved-answer tooling for rubric inspection.
  No answers are regenerated; no reasoning runs occur.
- No OpenAI API calls of any kind. Categorization sessions may use any agent;
  this is analysis, not a reasoning evaluation, but the $0-API rule still holds.

## Method

1. Enumerate every paired case-instance (case × draw) where the service scored
   fewer claims than `filesystem_sidecar`, and separately where it scored fewer
   than `filesystem`. Also enumerate the mirror set (service wins) — the
   autopsy must be symmetric or it becomes a narrative.
2. For each instance, read: the graded answer, the rubric slots missed, and the
   service session's operation record (open/query/read/checkpoint sequence,
   returned evidence, the 20 known failed local checkpoint commands across 17
   sessions).
3. Assign one primary cause (secondary optional) from this fixed taxonomy:
   - **T1 harness-friction** — checkpoint-syntax retries or other measured
     harness defects consumed turns/attention (17 sessions are pre-identified).
   - **T2 retrieval-miss** — no rubric-accepted source in returned context.
   - **T3 section-selection** — source present; decisive span absent.
   - **T4 exact-value serialization** — value present; paraphrased in answer.
   - **T5 claim-slot placement** — facts present; wrong slot.
   - **T6 grader-strictness** — substantively correct; matcher miss.
   - **T7 reasoning/synthesis** — evidence present and readable; wrong
     conclusion drawn.
   - **T8 other/unknown.**
4. Dual-rater protocol: two independent categorization passes by different
   sessions/agents; record raw inter-rater agreement; adjudicate disagreements
   with written one-line rationales. Report agreement before and after.
5. Repeat the tally restricted to the chronic-case set and the transitions
   suite (the only positive-CI cell), which get their own subsections.

## Deliverables

`results/2026-MM-DD-e12-loss-autopsy.{json,md}` containing: per-instance rows
(case, draw, arms, claims, primary/secondary cause, rationale ≤2 sentences);
taxonomy counts for losses and wins per comparison arm; the chronic and
transitions subsections; inter-rater agreement; and a decision section with
exactly three findings:

1. The share of the deficit attributable to T1 harness friction, and whether
   E01's headline point estimate deserves a recorded caveat.
2. Whether any surviving loss family is large and mechanistic enough to justify
   a NEW context-shaping design doc (none exists after the E04/E06 rejections;
   the burden of proof sits here, not in another feature bet).
3. The Tier B entry triage disposition required by D14: proceed / proceed with
   named mitigations / hold.

## Acceptance criteria

- 100% of losing case-instances categorized; mirror wins categorized at ≥50%
  sampling (all, if time permits).
- Dual-rater agreement reported; adjudication complete; no instance left T8
  without a one-line reason.
- The three decision findings answered explicitly.

## Cost preflight and ceiling

$0 OpenAI API (hard ceiling $0 — any API call is a protocol violation).
No Codex reasoning runs. Session time only.

## Abort criteria

None required — the experiment is read-only over immutable artifacts. If any
step would require regenerating an answer or calling a model API, stop and
record the gap instead.

## Reporting

The run record must name the exact input artifact hashes consumed (matching
the E01 ledger), both raters' identities/sessions, and the adjudication log.

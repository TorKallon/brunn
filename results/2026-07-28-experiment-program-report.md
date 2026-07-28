Created: 2026-07-28
Evidence snapshot: `e2df1915a6e01606c6246ab8448b1f78b4ce62bc`
Status: Interim program report; E06-E08 are still in progress

# E01-E11 experiment program report

## Executive conclusion

The checked-in evidence supports five completed experiment outcomes, three
in-progress experiments, and three prerequisite aborts.

- E01 completed the paired-draw baseline, but did not establish service
  non-inferiority or suite-level superiority. Its paired RuptureOps result also
  does not reproduce a service-over-files overfetch problem.
- E02, E03, E04, and E05 are definitive negative results for the tested
  implementations. Keep `verbatim_spans`, `semantic_lane`, all tested D01
  result-budget flags, and `lexical_single_scan` off.
- E06-E08 have no definitive result artifact in this snapshot. They remain
  in progress, with no verdict inferred from predeclarations or partial work.
- E09-E11 were not executed because their prerequisites were absent or failed.
  A prerequisite abort is not an experiment verdict and supports no positive
  or negative efficacy claim.

Across the completed and prerequisite-aborted evidence in this snapshot,
actual API-key reasoning spend is **$0**, actual embedding spend is **$0**, and
recorded ChatGPT-subscription-equivalent usage is **$254.88**. That equivalent
is an accounting measure, not an API invoice. E06-E08 are excluded from the
actual total until their definitive cost ledgers exist. Their frozen/planned
main grids total another $80.64 subscription-equivalent and expect $0
embeddings, so the $20 embedding-warning threshold is not implicated.

## Scorecard

This table has exactly one row for each experiment. `Not evaluated` means the
experiment produced no efficacy verdict.

| Experiment | Status and verdict semantics | Execution count | Load-bearing result | Feature/default decision | Actual API-key reasoning / embeddings / subscription-equivalent |
| --- | --- | --- | --- | --- | ---: |
| E01 | Complete; baseline machinery passed, parity non-inferiority not established | 531 definitive case-runs (59 cases × 3 arms × 3 draws), plus 3 excluded calibration runs | Service−sidecar claims −4.667, 95% CI [−13.667, 4.333], below the predeclared −5 lower-bound margin; RuptureOps service−filesystem chars −79,549, 95% CI [−111,508, −49,126] | No feature flip; retain the measured baseline and make no service-parity or overfetch superiority claim | $0 / $0 / $128.16 |
| E02 | Complete negative; tested D02 implementation rejected | 3 deterministic artifacts across 3 scales; 270 identifier probe-observations; 0 reasoning case-runs | Stage 1 and flag-off returned 0/30 at 1K, 10K, and 64K; flag-on returned only 4/30 at every scale and 0 deeper-offset probes | Keep `verbatim_spans=off`; repair D02 before rerunning E02 | $0 / $0 / $0 |
| E03 | Complete negative; current semantic-ready path rejected | 2 completed 64K modes × 30 samples; Mode 3 and quality backfill not run | Mode 1 passed 62/62 gates; Mode 2 passed 63/64 but had four ~2.5s timeout-shaped ordinary requests and failed the zero-deferred/unavailable-lane gate | Keep `semantic_lane=off`; repair the synchronous timeout path before paid Mode 3 or E09 | $0 / $0 / $0 |
| E04 | Complete negative; both tested D01 candidates rejected | 528/528 case-runs, 52 JSON/Markdown artifact pairs, 2 successful 640K soaks | RuptureOps char reduction was 1.26% for B and 6.78% for C versus the required 25%; chronic B was 3W/4L/5T and C 3W/3L/6T, each with one previously nonzero case at 0/5 | Retain Arm A; keep `search_fair_share`, `search_char_cap`, and `search_top1_hydration` off; close D01 in the tested form | $0 / $0 / $126.72 |
| E05 | Complete negative; lexical consolidation rejected | 2 successful 640K soaks; 795 paired selected search samples; 0 reasoning case-runs | All 795 selected SQL-statement deltas were exactly 0; the blocking comparator required at least one strict reduction | Keep `lexical_single_scan=off`; drop the deferred D10 item permanently | $0 / $0 / $0 |
| E06 | In progress; not evaluated in this snapshot | Definitive counts unavailable; frozen plan is 45 case-runs | No definitive result artifact is checked in; the predeclaration is not outcome evidence | Keep `resume_deltas=off` until E06 produces a valid verdict | — / — / — (planned main grid $10.80) |
| E07 | In progress; not evaluated in this snapshot | Definitive counts unavailable; frozen plan is 162 case-runs | No definitive result artifact is checked in; the draw-count predeclaration is not outcome evidence | Keep `supersession_demotion=off` until E07 produces a valid verdict | — / — / — (planned main grid $38.88) |
| E08 | In progress; not evaluated in this snapshot | Definitive counts unavailable; specified plan is 129 case-runs | No definitive result artifact is checked in; the experiment specification is not outcome evidence | Keep `intention_ledger=off` until E08 produces a valid verdict | — / — / — (planned main grid $30.96) |
| E09 | Prerequisite abort; experiment not executed and not evaluated | 0 stacks, 0 deterministic runs, 0 reasoning case-runs, 0 embedding requests | E03 Mode 2 failed its blocking gate and the semantic quality backfill was not run | Keep `semantic_lane=off`; repair and pass E03 before reconsidering E09 | $0 / $0 / $0 |
| E10 | Prerequisite abort; experiment not executed and not evaluated | 0 stacks, 0 deterministic runs, 0 reasoning case-runs, 0 embedding requests | No accepted immutable launch manifest existed; surviving feature qualifications and the E09 posture were unresolved | Keep the Tier C gate closed; freeze the accepted manifest only after all ship/drop outcomes exist | $0 / $0 / $0 |
| E11 | Prerequisite abort; experiment not executed and not evaluated | 0 stacks, 0 deterministic runs, 0 reasoning case-runs, 0 embedding requests | D02 was rejected, D06/`link_leads` was absent, and the required owner-authored manifest did not exist | Leave D06 unbuilt and `link_leads` unavailable; do not solicit the owner manifest until product prerequisites clear | $0 / $0 / $0 |

## Definitive results

### E01 — paired-draw machinery and baseline

All 531 definitive case-runs completed without a top-level error, timeout, or
nonzero final exit. The service made 520 actual HTTP operations and all returned
HTTP 200. The fair service-versus-sidecar comparison was −4.667 claims per
236-claim corpus draw with a case-clustered bootstrap 95% CI of
[−13.667, 4.333]. Because the lower bound was not above the predeclared
−5-claim margin, non-inferiority was not established; that does not prove the
service inferior.

Service-versus-filesystem was −3.333 claims, 95% CI [−13.667, 6.333].
RuptureOps service tool output averaged 63,090 characters per case versus
142,640 for filesystem, a paired difference of −79,549 with 95% CI
[−111,508, −49,126]. No suite met the two-part superiority rule.

Primary evidence:

- `results/2026-07-28-e01-aggregate.json` —
  `ae1bf01dfcb478b42edb7892ff1ddf38314a022180b0b7c065c502a35362d1db`
- `results/2026-07-28-e01-aggregate.md` —
  `32e99366225ec223350038e712a6adb3dde9cfc6b6e7932532910b5fb875f405`

### E02 — verbatim identifier gate

Stage 1 reproduced the original defect: 0/30 identifiers were returned at each
of 1K, 10K, and 64K. Stage 2 flag-off also returned 0/30 at each scale. Flag-on
returned only the four probes planted at byte offset 2600 at every scale and
returned no deeper probes. The flag-on search retained the same 11-statement
query count as flag-off, so latency and query-count safety did not rescue the
failed feature contract. The 640K soak and all 134 planned reasoning case-runs
were correctly aborted.

Primary evidence:

- `results/2026-07-27-e02-definitive-summary.json` —
  `a5060af37aac41634ae68267906ffb4856e621fb97c5770e3939276951c62318`

### E03 — semantic-ready latency profile

Exact/lexical Mode 1 passed all 62 gates at 64K. Semantic-ready owned-mock Mode
2 reached ready coverage with no pending or failed jobs and passed 63 of 64
gates, but four ordinary operations took approximately 2.5 seconds. It failed
`semantic_ready_runs_have_no_deferred_or_unavailable_lane`. Mode 3 and the
quality backfill were therefore not run. This is a negative result for the
current synchronous semantic-ready path, not for exact/lexical retrieval.

Primary evidence:

- `results/2026-07-27-e03-definitive-summary.json` —
  `52c9ff6835a3415a588828c2128d293b80b7ef61ddfb7dd6fd6588245da8cdf8`

### E04 — result budget experiment

Both B and C passed their 640K deterministic gates, and the top-1 hydration
comparator observed the expected +5 statements in all 795 selected search
samples with no non-search deltas. The efficacy tradeoff nevertheless failed.
B reduced cluster-weighted RuptureOps service-result characters from 59,975 to
59,222 (1.26%); C reduced them to 55,908 (6.78%). Neither reached the required
25% reduction or absolute 53,100-character target, and neither showed a
significant RuptureOps claim improvement. The chronic gate also failed for
both. Retain A and ship neither candidate.

Primary evidence:

- `results/2026-07-28-e04-definitive-summary.json` —
  `b92047dbf3273e3d90843a35e69751460c00fe3147b85133d4b5256e024abc99`
- `results/2026-07-28-e04-report.md` —
  `86b6207c5daf63c90d03d606b5165ef61360d2fe66d3735271f6eaa29959a81a`

### E05 — lexical consolidation guard

Both control and treatment passed their 640K soaks. The treatment returned all
30/30 old-source probes and 30/30 bounded-overflow probes, and unrelated-write
p95 was 13.683ms against a 58ms gate. However, every one of 795 paired selected
search samples had SQL-statement delta 0, and every unselected operation also
had delta 0. Because the predeclared comparator required at least one negative
delta, reasoning was prohibited and the candidate was rejected.

Primary evidence:

- `results/2026-07-28-e05-definitive-summary.json` —
  `f4c964d3dab3f18745a852c39879bcf4dde7378266e0c4edc0fbf51af7a5422a`
- `results/2026-07-28-e05-report.md` —
  `e95799a86e13ebad4cb24ca09567ae71ec4cdc2834d247327fff306e83a28664`

## In-progress placeholders

E06-E08 are deliberately placeholders. Their checked-in predeclarations and
specification establish intended grids and cost ceilings, not observed
execution counts, costs, results, or verdicts.

- E06 plan evidence:
  `results/2026-07-28-e06-grid-predeclaration.json`,
  `9e0983315dd1b7f1ab9c63e4db3ee4c86c4bbc4d4bdf6927bf68e3cd02365dc5`
- E07 plan evidence:
  `results/2026-07-28-e07-draw-count-predeclaration.json`,
  `95a05fbb66f3c600d406c1fed088a0b305f985b41d105c9007ac1463b4616050`
- E08 specification evidence:
  `docs/mvp/E08-intention-ledger-experiment.md`,
  `b84a13908ecd911cb0569a25e68f71e5e3e063f5854720a2f42038682cb4839a`

## Prerequisite aborts

E09, E10, and E11 each have `experiment_executed=false` and
`experiment_verdict=not_evaluated`. Their evidence records zero stacks,
deterministic runs, reasoning case-runs, embedding requests, and result
artifacts.

- E09 is blocked by E03 Mode 2's failed gate and the absent quality backfill.
  Its future preflight estimates $0.84 expected and $2.00 ceiling embedding
  spend, both below the $20 warning threshold.
- E10 is blocked by the absent accepted immutable launch manifest and
  unresolved surviving feature/posture decisions. Under the then-specified
  semantic-off posture, its future embedding estimate is $0.
- E11 is blocked independently by rejected D02, absent D06/`link_leads`, and
  the absent owner-authored manifest. Its optional owner-corpus indexing
  estimate is $0.19, not incurred.

Primary evidence:

- `results/2026-07-28-e09-prerequisite-abort.json` —
  `a18980a2e1dbfc373b3aac4fa91b01a40abe582a5edac12d6f33e750674f0d4f`
- `results/2026-07-28-e10-prerequisite-abort.json` —
  `6581cd5faa3d3769426594fc727a9ed3e2f88688849ac07acaa954225520c921`
- `results/2026-07-28-e11-prerequisite-abort.json` —
  `8632bcfc5f79647086f342104664993670c8585ae05dfcd17f97a1cc07228976`

## Evidence-audit notes

- The E10 prerequisite-abort artifact is a truthful snapshot at
  `d989ae5893e24a14d11d51ad9c4cfc8c8b812e1b`, but its
  `feature_qualifications` blocker includes E04 as unresolved. E04 was
  subsequently completed and rejected both candidates. E10 remains blocked
  because E06-E08 and E09 still prevent an accepted immutable launch manifest.
- E01's aggregate field `draws=15` counts five suite artifacts across three
  repeated draws; it does not mean 15 repeated draws per case. The definitive
  execution count is 59 cases × 3 arms × 3 repeated draws = 531 case-runs.
- E03's overall verdict is negative even though Mode 1 passed. The blocking
  semantic-ready Mode 2 result controls the semantic-lane decision.
- No predeclaration, partial deterministic run, or prerequisite abort has been
  promoted to an experiment verdict in this report.

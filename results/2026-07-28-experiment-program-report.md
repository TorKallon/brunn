Created: 2026-07-28
Evidence snapshot: `3122f15bb740b0c92e124cc80b42201370c84bae`
Status: Final

# E01-E11 experiment program report

## Executive conclusion

The checked-in evidence supports seven completed experiment records, one
deterministic-preflight stop, and three prerequisite aborts.

- E01 completed the paired-draw baseline, but did not establish service
  non-inferiority or suite-level superiority. Its paired RuptureOps result also
  does not reproduce a service-over-files overfetch problem.
- E02-E06 are five definitive negative results for their tested
  implementations. Keep `verbatim_spans`, `semantic_lane`, all tested D01
  result-budget flags, `lexical_single_scan`, and `resume_deltas` off.
- E07 is a split completed result. Its explicit-supersession mechanism passed
  the frozen mechanism rule, but the estimate is imprecise and unprompted
  adoption failed. Keep D04 default-off and add assisted authoring before a
  new adoption qualification.
- E08 stopped at deterministic preflight because flag-on concurrent-search p95
  was 874.535ms against a 750ms gate. No feature-comparison arm ran, so E08 is
  not evaluated and D05 has no ship or kill verdict.
- E09-E11 were not executed because their prerequisites were absent or failed.
  A prerequisite abort is not an experiment verdict and supports no positive
  or negative efficacy claim.

Across all recorded evidence, actual API-key reasoning spend is **$0**, actual
embedding spend is **$0**, and recorded ChatGPT-subscription-equivalent usage
is **$304.56** for 1,269 plan-backed sessions. The 1,269 includes E01's three
excluded calibration sessions; 1,266 case-runs contribute to definitive
experiment records. The equivalent is an accounting measure, not an API
invoice. E08's unrun grid would have been $30.96, but that amount was not
incurred and no rerun under the frozen protocol is authorized. The $20
embedding-warning threshold was never approached.

## Scorecard

This table has exactly one row for each experiment. `Not evaluated` means the
experiment produced no efficacy verdict.

| Experiment | Status and verdict semantics | Execution count | Load-bearing result | Feature/default decision | Actual API-key reasoning / embeddings / subscription-equivalent |
| --- | --- | --- | --- | --- | ---: |
| E01 | Complete; baseline machinery passed, parity non-inferiority not established | 531 definitive case-runs (59 cases × 3 arms × 3 draws), plus 3 excluded calibration runs | Service−sidecar claims −4.667, 95% CI [−13.667, 4.333]; the CI lower bound did not exceed the predeclared −5 margin. RuptureOps service−filesystem chars −79,549, 95% CI [−111,508, −49,126] | No feature flip; retain the measured baseline and make no service-parity, inferiority, or overfetch superiority claim | $0 / $0 / $128.16 |
| E02 | Complete negative; tested D02 implementation rejected | 3 three-scale measurement artifacts plus 1 query-budget calibration; 270 identifier probe-observations; 0 reasoning case-runs | Stage 1 and flag-off returned 0/30 at 1K, 10K, and 64K; flag-on returned only 4/30 at every scale and 0 deeper-offset probes | Keep `verbatim_spans=off`; repair D02 before rerunning E02 | $0 / $0 / $0 |
| E03 | Complete negative; current semantic-ready path rejected | 2 completed 64K modes × 30 samples; Mode 3 and quality backfill not run | Mode 1 passed 62/62 gates; Mode 2 passed 63/64 but had four ~2.5s timeout-shaped ordinary requests and failed the zero-deferred/unavailable-lane gate | Keep `semantic_lane=off`; repair the synchronous timeout path before paid Mode 3 or E09 | $0 / $0 / $0 |
| E04 | Complete negative; both tested D01 candidates rejected | 528/528 case-runs, 52 JSON/Markdown artifact pairs, 2 successful 640K soaks | RuptureOps char reduction was 1.26% for B and 6.78% for C versus the required 25%; chronic B was 3W/4L/5T and C 3W/3L/6T, each with one previously nonzero case at 0/5 | Retain Arm A; keep `search_fair_share`, `search_char_cap`, and `search_top1_hydration` off; close D01 in the tested form | $0 / $0 / $126.72 |
| E05 | Complete negative; lexical consolidation rejected | 2 successful 640K soaks; 795 paired selected search samples; 0 reasoning case-runs | All 795 selected SQL-statement deltas were exactly 0; the blocking comparator required at least one strict reduction | Keep `lexical_single_scan=off`; drop the deferred D10 item permanently | $0 / $0 / $0 |
| E06 | Complete negative; tested D03 implementation rejected | 45/45 case-runs (3 arms × 5 cases × 3 draws), 9 JSON/Markdown pairs, 2 successful deterministic artifacts | B scored 34/60 claims vs A 33/60 and C 40/60, completed 0/5 cases in every draw, and enlarged all 15 paired resume payloads by 63,387 chars total | Keep `resume_deltas=off`; close D03 in its tested form | $0 / $0 / $10.80 |
| E07 | Complete split result; mechanism passed, deployment adoption failed | 162/162 sessions: 126 claim-scored plus 36 adoption | Family 6W/2L/4T, net +4 ≥ +3; clustered delta +1.333, CI [−2.333, 3.667]; supersession adoption 7/18 < 50% | Keep `supersession_demotion=off`; add assisted authoring and requalify adoption before Tier C | $0 / $0 / $38.88 |
| E08 | Stopped at deterministic preflight; feature not evaluated | 1 flag-on 64K/30 calibration; 52/54 gates green; 0 query contracts, latency contrasts, reasoning case-runs, draws, or audits | Concurrent-search p95 874.535ms > 750ms; the other red gate was the expected calibration-only ineligibility gate | No D05 verdict; keep `intention_ledger=off`; require a prospectively amended protocol before rerun | $0 / $0 / $0 ($30.96 planned, not incurred) |
| E09 | Prerequisite abort; experiment not executed and not evaluated | 0 stacks, 0 deterministic runs, 0 reasoning case-runs, 0 embedding requests | E03 Mode 2 failed its blocking gate and the semantic quality backfill was not run | Keep `semantic_lane=off`; repair and pass E03 before reconsidering E09 | $0 / $0 / $0 |
| E10 | Prerequisite abort confirmed; experiment not executed and not evaluated | 0 stacks, 0 deterministic runs, 0 reasoning case-runs, 0 embedding requests | E04 and E06 are now resolved drops, but no accepted manifest exists; E07 adoption, E08/D05, and E09 still block it | Keep the Tier C gate closed; freeze the accepted manifest only after all ship/drop outcomes exist | $0 / $0 / $0 |
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

### E06 — resume delta experiment

All 45 frozen case-runs completed successfully across three arms, five cases,
and three draws. Both deterministic artifacts passed. The treatment's resume
p95 was 77.606ms against a 150ms gate, its paired resume-query delta was
exactly +5 across 30 samples, and all 30 lineage samples passed.

Those mechanical results did not transfer to task quality or payload
discipline. Treatment B passed 34/60 claims versus 33/60 for A and 40/60 for C,
but it completed zero of five cases in every draw. Its one-sided exact McNemar
p-values were 0.8125 versus A and 0.96875 versus C. The 15 operation-level
resume comparisons all violated payload neutrality: control totaled 355,921
result characters, treatment totaled 419,308, and treatment added 63,387
characters overall, 4,225.8 per pair on average. Pair deltas ranged from 3,159
to 5,436 characters.

D03 is rejected in its tested form. Keep `resume_deltas` default-off and do not
advance it to a Nyx rollout.

Primary evidence:

- `results/2026-07-28-e06-definitive-summary.json` —
  `a9972cfa44f97c6f11bb2cc4181bfeeff4ec8ae00394edf61846aefd04263eb4`
- `results/2026-07-28-e06-report.md` —
  `053fd1035084abdfdbf4ce136c33f973acce7186f45a3b89171650bcbca87254`

### E07 — supersession mechanism and adoption

All 162 frozen sessions completed cleanly: 126 claim-scored runs across flag,
baseline, and filesystem, plus 36 unprompted-adoption sessions. There were no
timeouts, record errors, or nonzero final exits. The flag-on deterministic
artifact passed its reviewed query contract and recorded foreground-write p95
of 27.401ms against a 58ms cap.

Within the four-case supersession family, flag produced six paired
case-instance wins, baseline produced two, and four tied. Net +4 clears the
frozen +3 rule. Flag passed 29/48 family claims versus 25/48 for baseline.
Safety and context gates also passed: no new forbidden assertions appeared
outside the family, and flag/base model-visible character ratio was
1.00812659 against the 1.05 cap.

The mechanism result is encouraging but imprecise. The case-clustered expected
per-draw claim delta was +1.333 with 95% CI [−2.333, 3.667].
Majority-collapsed full-case outcomes had zero discordant cases and exact
McNemar p=1.0. E07 passes the predeclared net-win rule; it does not establish a
precise nonzero effect.

The separate deployment-adoption gate failed. Only 7/18 eligible sessions
emitted syntactically valid `supersedes` frontmatter, 38.889% against a 50%
minimum. A hash-bound semantic companion found 6/18 resolvable distinct edges,
5/18 selected projected edges, and 3/18 selected emissions without a competing
successor. The same adoption cohort produced intention frontmatter in 13/18
sessions, but that separate positive measure neither repairs D04 nor creates an
E08 verdict.

The D04 retrieval mechanism is accepted under E07's frozen mechanism rule, but
D04 remains default-off and is not Tier-C ready. Add assisted authoring, reject
self-edges in adoption measurement, surface parallel-successor ambiguity, and
run a new adoption qualification.

Primary evidence:

- `results/2026-07-28-e07-aggregate.json` —
  `3d2d52159383c196930924b0e75e98c631dbdd7b61bb5d8bc5a830ffdd471f40`
- `results/2026-07-28-e07-aggregate.md` —
  `29f0d2fe1bc1a6ef00f8667a275fbf3c7a0caa5b68fab73b19f0f9981a6bfcef`
- `results/2026-07-28-e07-adoption-aggregate.json` —
  `d12c593f3a647861503d735fd5b875fb3a5adca4b6b89b84ffd858146ab1acba`
- `results/2026-07-28-e07-conclusion-audit.json` —
  `f2eecc31b3ec4df4db622db04f570bf2710ddb75b82c3d96c87625a844be72ed`
- `results/2026-07-28-e07-adoption-semantic-audit.json` —
  `a2b0435f6e4bbea56a41058f84abc3f31bc1ea1e41dfc817c87ce04bbb8ad7c9`
- `results/2026-07-28-e07-session-health-audit.json` —
  `07b2a58dbfa7b7cc2ae9a3505dc00d4b5fd84414c89cec9d028be5070b546243`

### E08 — deterministic-preflight stop

E08 did not reach a feature comparison. Its one flag-on 64K/30 query-budget
calibration passed 52 of 54 gates and recorded every canonical query-count
sample, but concurrent-search p95 was 874.535ms against the 750ms regression
ceiling. The other red gate,
`query_budget_calibration_is_not_acceptance`, is expected for every
calibration artifact.

Canonical counts were complete: open was 17 in 29 samples and 22 once; search
was 11 in all 30; read was 11 in all 30; write was 14 in all 30; checkpoint
was 28 once; and resume was 32 in 29 samples and 37 once. Complete counts are
necessary but insufficient when another deterministic gate is red.

No `e08-intention-ledger` query-budget contract was authored. Zero of two
latency-contrast arms, 15 planned reasoning invocations, 129 planned case-runs,
three draws, or the interim and final audits ran. E08 therefore says nothing
definitive about prospective-memory quality, false surfacing,
non-prospective regression, pointer limits, or open-latency delta.

This is a deterministic-preflight stop, not a D05 feature pass or failure.
Keep `intention_ledger` default-off. A rerun requires a prospectively approved
amended protocol; the frozen protocol is not authorized for a simple retry.
The unrun grid's $30.96 subscription-equivalent was not incurred.

Primary evidence:

- `results/2026-07-28-e08-query-budget-calibration.json` —
  `d0780b4fb5d705ca09f4630cd9dad63b50dfa8b451b1e877964902ff33240084`
- `results/2026-07-28-e08-deterministic-preflight-stop.json` —
  `951003bdf16498e8c406adf34d189cfac85c0e98531fd4790f14c9828f0642b2`
- `results/2026-07-28-e08-report.md` —
  `700ef0cc96bf21e2d48ca63c624b0dbf2c04158df7d71f580c05ffc2789bf3b2`

## Prerequisite aborts

E09, E10, and E11 each have `experiment_executed=false` and
`experiment_verdict=not_evaluated`. Their evidence records zero stacks,
deterministic runs, reasoning case-runs, embedding requests, and result
artifacts from an efficacy run.

- E09 is blocked by E03 Mode 2's failed gate and the absent quality backfill.
  Its future preflight estimates $0.84 expected and $2.00 ceiling embedding
  spend, both below the $20 warning threshold.
- E10's current audit resolves E04/D01 and E06/D03 as drops, alongside the
  already rejected D02 and `lexical_single_scan` candidates. It remains blocked
  by the absent accepted immutable launch manifest, E07's failed adoption
  qualification, E08's absent D05 verdict, and E09's prerequisite-aborted
  semantic posture. Under the specified semantic-off posture, its future
  embedding estimate is $0.
- E11 is blocked independently by rejected D02, absent D06/`link_leads`, and
  the absent owner-authored manifest. Its optional owner-corpus indexing
  estimate is $0.19, not incurred.

Primary evidence:

- `results/2026-07-28-e09-prerequisite-abort.json` —
  `a18980a2e1dbfc373b3aac4fa91b01a40abe582a5edac12d6f33e750674f0d4f`
- `results/2026-07-28-e10-prerequisite-current-audit.json` —
  `ca97445141eede8cfe09cc107dd0226c981d998ac078915abcd3693a5dfdb65c`
- `results/2026-07-28-e10-prerequisite-abort.json` —
  `6581cd5faa3d3769426594fc727a9ed3e2f88688849ac07acaa954225520c921`
- `results/2026-07-28-e11-prerequisite-abort.json` —
  `8632bcfc5f79647086f342104664993670c8585ae05dfcd17f97a1cc07228976`

## Evidence-audit notes

- The original E10 prerequisite-abort artifact is a truthful snapshot at
  `d989ae5893e24a14d11d51ad9c4cfc8c8b812e1b`. The current audit preserves it
  unchanged, records E04 and E06 as resolved drops, and identifies E07
  adoption, E08/D05, E09, and the absent accepted manifest as the current
  blockers.
- E01's aggregate field `draws=15` counts five suite artifacts across three
  repeated draws; it does not mean 15 repeated draws per case. The definitive
  execution count is 59 cases × 3 arms × 3 repeated draws = 531 case-runs.
- E03's overall verdict is negative even though Mode 1 passed. The blocking
  semantic-ready Mode 2 result controls the semantic-lane decision.
- E07's mechanism pass and adoption failure are separate results. The wide
  confidence interval and McNemar p=1.0 are reported as uncertainty, not as
  proof of a precise nonzero effect.
- E08's partial deterministic run is not promoted to a D05 feature verdict,
  and no prerequisite abort is promoted to an efficacy verdict.

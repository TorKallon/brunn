# E09–E11 prerequisite-abort record

Recorded: 2026-07-28
Source revision: `d989ae5893e24a14d11d51ad9c4cfc8c8b812e1b`

These are prerequisite outcomes, not experiment results. E09, E10, and E11
were not executed: no service stack, Docker project, deterministic measurement,
reasoning case-run, or embedding request was started for any of them.

## Results and conclusions

| Experiment | Recorded result | Exact blockers | Conclusion |
|---|---|---|---|
| E09 | Prerequisite abort; **not evaluated** | E03 semantic-ready Mode 2 failed its blocking zero-deferred-or-unavailable-lane gate (63/64 gates, four timeout-shaped observations near 2.5 seconds); the eval-corpus quality backfill was not run | Make no ship/bound/cut claim. Keep `semantic_lane` default-off, repair and pass free Mode 2, then complete the quality backfill before E09 |
| E10 | Prerequisite abort; **not evaluated** | No accepted immutable launch flag manifest; E04 and E06–E08 have not collectively resolved the feature set at this source snapshot; E09 has no decided semantic posture | Make no Tier C launch claim. Keep the cutover gate closed until all surviving features have accepted outcomes and the exact manifest is frozen and hashed |
| E11 | Prerequisite abort; **not evaluated** | D02 rejected; D06/`link_leads` implementation absent; `eval/owner_link_cases.json` and owner sign-off absent | Make no D06 ship/kill claim. Leave D06 unbuilt; do not solicit owner cases while the independent product blockers still prohibit a run |

E01 is complete and is not an E10 blocker. Its definitive aggregate is
`results/2026-07-28-e01-aggregate.json`
(`ae1bf01dfcb478b42edb7892ff1ddf38314a022180b0b7c065c502a35362d1db`).
Two rejected features already constrain any future E10 manifest:
`verbatim_spans=off` from E02 and `lexical_single_scan=off` from E05.

## Evidence

- E02 definitive summary:
  `results/2026-07-27-e02-definitive-summary.json`
  (`a5060af37aac41634ae68267906ffb4856e621fb97c5770e3939276951c62318`).
  D02 flag-on returned 4/30 at 1K, 10K, and 64K and returned no deeper
  identifier probes.
- E03 definitive summary:
  `results/2026-07-27-e03-definitive-summary.json`
  (`52c9ff6835a3415a588828c2128d293b80b7ef61ddfb7dd6fd6588245da8cdf8`).
  Mode 2 failed; paid Mode 3 and the eval-corpus quality backfill are recorded
  as not run.
- E05 definitive summary:
  `results/2026-07-28-e05-definitive-summary.json`
  (`f4c964d3dab3f18745a852c39879bcf4dde7378266e0c4edc0fbf51af7a5422a`).
  The treatment produced zero strict SQL-statement reductions across 795
  paired search samples, so `lexical_single_scan` was rejected before
  reasoning.
- At the recorded source revision, exact source searches under `apps/` and
  `eval/` found zero files containing `link_leads` or `linked_leads`;
  `eval/owner_link_cases.json` is neither present nor tracked.

## Future cost ledgers

These estimates are unincurred. Reasoning must use the owner's
ChatGPT-authenticated Codex subscription, fail-closed; only embeddings may use
the usage-billed API.

| Experiment | Base reasoning plan | Bounded contingency | Hard ceiling | Embedding plan |
|---|---:|---:|---:|---:|
| E09 | 351 case-runs / $84.24 equivalent | 36 case-runs / +$8.64 equivalent | $100 equivalent | ~$0.84 expected; $2 conservative ceiling |
| E10 | 354 case-runs / $84.96 equivalent | optional draw: 118 / +$28.32 equivalent | $120 equivalent | $0 under the specified semantic-off posture |
| E11 | 168 case-runs / $40.32 equivalent | $9.68 equivalent retry headroom | $50 equivalent | $0 required; optional owner-corpus indexing ~$0.19 |

Actual across all three aborts: **$0 usage-billed reasoning, $0 embeddings,
$0 subscription-equivalent inference, and 0 case-runs**. The embedding
notification threshold is $20; no estimate approaches it and no warning was
triggered.

Machine-readable records:

- `results/2026-07-28-e09-prerequisite-abort.json`
- `results/2026-07-28-e10-prerequisite-abort.json`
- `results/2026-07-28-e11-prerequisite-abort.json`

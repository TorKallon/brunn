# Straylight Alpha Candidate Comparison

Status: complete

This report compares the 2026-07-23 hardening candidate with fresh direct
filesystem controls and the saved 2026-07-22 Straylight service baseline. The
read, write, capture, dreaming, retrieval, and token-budget semantics were not
changed by the hardening work.

## Complete Harness

| Suite | Filesystem cases | Straylight cases | Filesystem claims | Straylight claims | Total input delta | Uncached input delta | Elapsed delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Main work | 13/13 | 13/13 | 52/52 | 52/52 | +34.4% | -5.1% | +47.6% |
| Personal coordination | 15/15 | 15/15 | 60/60 | 60/60 | +9.2% | +30.7% | +17.7% |
| Rupture Ops | 9/12 | 11/12 | 43/48 | 47/48 | -25.2% | -40.9% | +2.1% |
| Changed-evidence resume | 5/5 | 5/5 | 20/20 | 20/20 | -19.0% | -26.8% | -4.3% |
| **Weighted total** | **42/45** | **44/45** | **175/180** | **179/180** | **+0.7%** | **-11.8%** | **+18.5%** |

Across all 45 paired cases, Straylight recovered four more deterministic
claims and passed two more complete cases. Cumulative input was effectively at
parity, uncached input was 11.8% lower, and completed agent tool calls were
15.1% lower. Mean elapsed time was 18.5% higher, concentrated in the broad main
suite.

The main-suite token and latency increase was dominated by one stochastic
agent run. `warmind-final-validation-handoff` made nine calls instead of the
saved baseline's four after following a reference to a document absent from
both frozen corpora. The API correctly returned `ref_not_found`; the agent then
performed extra searches. Other main-suite packet sizes were substantially
unchanged.

## Quality Interpretation

The one Straylight Rupture Ops miss is a claim-slot grading artifact, not a
missing fact or source. In two independent runs, the answer correctly included:

- map markers represent possible persistent population, not simultaneous
  attackers;
- valuable supplies should be banked when practical;
- replacement equipment belongs in a safe recovery cache;
- a Regeneration Chamber does not guarantee local recovery after corruption;
- both site threat assessments and the exact guaranteed chest value.

The evaluator required the marker and recovery-cache language to be repeated
inside claim `c3`; the agent placed those facts in the adjacent `c1` and `c2`
claims. The original full-suite result remains unchanged and is reported as
47/48. A targeted rerun is retained separately and confirms that retrieval,
citations, recommendation, and checkpoint persistence were complete.

The initial transition run exposed two missing equivalent wording forms in the
evaluator (`August 22–25` and `supersedes`). The pre-fix result is retained.
After adding only those accepted grammatical variants, the complete suite
passed 5/5 cases and 20/20 claims in both conditions.

## Saved Service Baseline

Compared with the saved 2026-07-22 Straylight runs, the weighted hardening
candidate used 2.0% less cumulative input and 3.9% less uncached input. Main and
personal claim quality was unchanged; transition quality remained 20/20. The
Rupture Ops deterministic score moved from 48/48 to 47/48 because of the
claim-slot behavior described above, while the underlying retrieved evidence
and answer remained complete.

There is no evidence that the P0/P1 security, account lifecycle, quota,
observability, deployment, or recovery changes degraded retrieval or reasoning
semantics. Repeated blinded holdouts are still required to characterize model
variance and tail latency before making stronger statistical claims.

## Candidate Artifacts

- `results/2026-07-23-alpha-candidate-main.json`
- `results/2026-07-23-alpha-candidate-personal.json`
- `results/2026-07-23-alpha-candidate-rupture.json`
- `results/2026-07-23-alpha-candidate-rupture-poi-rerun.json`
- `results/2026-07-23-alpha-candidate-transitions.json`
- `results/2026-07-23-alpha-candidate-transitions-pre-rubric-fix.json`

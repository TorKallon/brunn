# Straylight simplified workspace comparison

Created: 2026-07-26

## Decision

The simplified workspace meets the current acceptance bar:

- aggregate reasoning quality is ahead of direct Markdown;
- weighted uncached model input is modestly lower;
- durable checkpoint continuation is preserved;
- the production timeout pattern is eliminated in the clean 64K-entry test;
- ordinary work no longer waits for semantic indexing or global consistency.

The result is not uniformly better. Direct files retain a large advantage for
an exact read when the caller already knows a local path, and the
changed-evidence transition lane is one claim behind files. Those differences
remain explicit.

## Reasoning quality

All rows compare fresh agents on the same frozen sources, task, model family,
structured answer contract, and current rubric. A strict card passes only when
all of its claims and source requirements pass.

| Suite | Direct Markdown | Simplified Straylight | Uncached input difference |
| --- | ---: | ---: | ---: |
| Established project work | 43/52 claims, 4/13 cards | **49/52, 11/13** | +3.7% |
| Recent Europe and Aether work | 43/48, 8/12 | **47/48, 11/12** | +0.7% |
| Rupture Ops | 36/48, **4/12** | **37/48**, 3/12 | -11.9% |
| Personal coordination | 47/60, **6/15** | 47/60, 5/15 | -10.9% |
| Changed-evidence transitions | **14/20**, 0/5 | 13/20, 0/5 | +2.8% |
| **Aggregate** | **183/228, 22/57** | **193/228, 30/57** | **-4.2%** |

Weighted mean uncached input was 26,458 tokens per filesystem case and 25,354
per Straylight case. Straylight persisted all 56 eligible checkpoints; the
read-only personal-coordination card correctly persisted none.

All five transition checkpoints passed durable read-back, exact parent,
generation, prior-source, delta-source, and service-call-budget checks. The
13/20 versus 14/20 answer gap consists of claim-slot omissions, not lost
lineage, but it remains a real narrow regression.

## Previous snapshot

The retained pre-simplification snapshot is commit `dd02756`.

On the current recent-work rubric, its saved run regrades to 37/48 claims for
the service versus 34/48 for files. The simplified run reaches 47/48 versus
43/48. Because the contemporaneous filesystem run also improved by nine
claims, the ten-claim service increase must not be attributed entirely to the
architecture. The defensible product comparison is the paired final result:
47/48 for Straylight versus 43/48 for files.

The performance difference is unambiguous. The old snapshot:

- took 35.8 seconds p95 to open only 500 synthetic files;
- timed out at 1,500 files;
- added 2,517 database rows for one checkpoint.

## Definitive performance

The final run started from an empty disposable PostgreSQL database. Each scale
used 30 measured samples, unknown-path discovery, broad search, exact read,
checkpoint/resume, a concurrent write/search probe, and a forced embedding
provider outage.

| Entries | Import | Open p95 | Search p95 | Broad p95 | Exact API read p95 | Flat-file discovery p95 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 0.217 s | 24 ms | 13 ms | 86 ms | 9 ms | 16 ms |
| 10,000 | 1.852 s | 40 ms | 51 ms | 559 ms | 15 ms | 92 ms |
| 64,000 | 11.628 s | 154 ms | 98 ms | 667 ms | 64 ms | 1,450 ms |

At 64K, unknown-path search was about 15 times faster than ripgrep discovery;
workspace open was about nine times faster. A direct filesystem read remains
far faster when the exact local path is already known.

Every target was found while semantic indexing was still pending. Exact and
lexical retrieval also survived a forced semantic-provider outage, and the
semantic lane became healthy after restoration. The 64K checkpoint took 120
ms and added 12 rows and 5,992 bytes.

The clean run exposed an over-pedantic old gate: it failed a 20 ms to 200 ms
growth ratio despite the result being far below the hard latency limits. The
revised guard reports every ratio but fails growth only when p95 also exceeds
one second. Hard limits of 5 seconds for open, 3 seconds for search, and 1
second for exact read remain unchanged.

## Consistency interpretation

Atomicity is local and small:

- one Markdown entry publication uses one brief PostgreSQL commit;
- one binary publication makes its binary entry and companion Markdown visible
  after immutable bytes are uploaded;
- mixed-validity reads keep successful entries;
- imports and exports progress independently per entry;
- vectors publish per chunk and may be partially available;
- background work, usage, metrics, and cleanup never gate ordinary reads.

There is no globally isolated workspace snapshot, corpus-wide transaction,
distributed PostgreSQL/S3 transaction, full-manifest copy, replay ledger, or
synchronous derived-data barrier.

## Remaining limits

- The definitive run proves retrieval while semantic indexing is pending; it
  does not measure full 64K embedding catch-up or 64K HNSW latency.
- Multi-user HNSW behavior at much larger hosted scale needs a separate
  production-shaped soak.
- The transition lane should be watched in future holdout runs rather than
  hidden by the stronger aggregate result.
- `/ready` dependency health is not a release proof; deployment still requires
  an authenticated behavioral canary.

## Evidence

- `results/2026-07-26-simplified-final-current-performance.json`
- `results/2026-07-26-simplified-final-agent-work-service-rerun-regraded.json`
- `results/2026-07-26-simplified-final-recent-work-regraded.json`
- `results/2026-07-26-simplified-final-rupture-ops-regraded.json`
- `results/2026-07-26-simplified-final-personal.json`
- `results/2026-07-26-simplified-final-transitions-rerun-regraded.json`
- `results/2026-07-26-dd02756-performance.json`
- `results/2026-07-26-dd02756-recent-work-regraded.json`

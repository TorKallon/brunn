# Brunn Production Platform Evaluation

Status: **Railway selected; operational cutover and repository publication passed**
Date: 2026-07-31

## Current decision

The earlier recommendation to use Render was superseded by the owner's Railway
selection. Railway now hosts the public web edge, private simplified API,
separate worker, and PostgreSQL. Production objects remain in an external,
versioned S3 store because Railway's native bucket contract does not provide
the versioning boundary Brunn requires. Nyx is operator/test/restore
infrastructure, not a second production deployment.

The current API is healthy and ready at build
`39761166d21b0cfa44d11e3ba18a52112693d0cd`, and all 56 migrations are applied.
The layered owner-data migration and zero-diff service/export audits pass.
Codex and Aether/OpenClaw pass their final client canaries, and the final web
deployment passes. Guarded backfill completed all 12,727 initial jobs with zero
queued, running, or failed and zero missing embeddings. The permanent
one-replica worker passes; repository publication is the only live item.

## Required platform contract

The owner deployment requires:

- Dockerized Rust API and independently stoppable worker;
- PostgreSQL with pgvector and a verified migration ledger;
- external versioned S3 object storage;
- private service networking with only the web proxy public;
- immutable release identity on API, worker, and web;
- health/readiness, rollback, checksummed backup, and isolated restore; and
- content-free operational metrics without exposing owner data.

## Historical comparison

This table records the 2026-07-27 evaluation; its recommendation is historical,
not current state.

| Platform | Historical strengths | Historical drawbacks | 2026-07-31 disposition |
| --- | --- | --- | --- |
| Render | Managed Postgres, private services, HTTPS log streams, declarative Blueprints | External S3 still required; strongest database protections are plan-dependent | Not selected |
| Railway | Fast Docker service setup, private networking, convenient environment references | Service-by-service Compose translation; database template operational ownership; native buckets lack the required versioning/SSE contract; observability needs extra work | **Selected**, with external versioned S3 and explicit backup/restore controls |
| Fly.io | Docker portability, regional placement, managed Postgres options | More infrastructure and log-shipping work | Not selected |
| AWS ECS/Fargate | Deepest RDS/S3/IAM/KMS/Datadog integration | Highest initial and ongoing operational burden | Deferred |

## Why Railway is acceptable for this owner cutover

The earlier assessment called Railway a poor fit if its native database bucket
and observability defaults were treated as the entire safety system. The live
design closes the most important gaps outside those defaults:

- objects live in external versioned S3;
- the database is captured as a checksummed PostgreSQL dump before migration;
- API and worker are separate, and the worker was held out of fidelity import;
- the API remains private behind the web proxy;
- exact source/history audits are application-level rather than provider claims;
  and
- Nyx remains available for an isolated restore drill without accepting live
  production writes.

An isolated restore was attempted, but locked Nyx prevented Docker daemon
access and no restore container was created. The backup checksum and catalog
still pass. This is recorded as an environment-blocked, non-blocking exception
for the direct owner cutover, not as recovery proof; the drill remains valuable
future operational evidence.

The owner approved Railway Pro; the subscription update is confirmed and its
$20/month minimum is infrastructure spend, not embedding spend. The database
volume was live-resized from 5 GB to 20 GB, matching the checked-in 20,000 MB
declaration. The final filesystem is 25% used with 13.6 GiB free.

## Current qualification record

| Check | State |
| --- | --- |
| Simplified API health/readiness | Pass |
| 56/56 migrations | Pass |
| External versioned S3 | Pass for import/export fidelity |
| Pre-cutover PostgreSQL dump/catalog validation | Pass |
| Public admin-route isolation | Pass |
| Historical owner-data zero-diff audit on Railway | Pass: 4,926 paths / 4,955 legacy versions / 5,079 native records |
| Full-history export | Pass: 20,047 copies / 797,775,263 bytes / zero differences |
| Fresh-source overlay audit | Pass: 4,267 files; all-skip replay; ten history-preserving soft deletions |
| Matching final API/worker/web identity | Pass; permanent worker deployment `7af78da7-3b01-4a66-9923-3aa8184d1978` is `SUCCESS` at exactly one replica and prior worker deployments are removed |
| Guarded embedding queue | Pass: 12,727 initial to zero queued/running/failed; 126,536 search chunks and zero missing embeddings |
| One-replica performance | Pass: 30 opens + 30 exact searches, zero failures; p95 31.809529 ms open and 29.295206 ms search against 120/107 ms limits |
| Production volume | Pass: live and IaC 20 GB; 18.3 GiB filesystem, 4.6 GiB used, 13.6 GiB free, 25% |
| Storage efficiency | Follow-up recommended: two unused HNSW indexes are distinct derived/rebuildable accelerators, not authoritative data; neither was dropped |
| Final focused verification | Pass: 79 targeted, 28 MCP, 18 web, and 10 Railway contract tests |
| Monitor synthetic-fault qualification | Deferred outside the owner cutover completion set |
| PostgreSQL plus S3 restore drill | Not performed: locked Nyx blocked Docker before a container was created; non-blocking exception for this direct cutover |
| Repository publication | Pass: evidence commit `dff91a210293483d95c9ea61c7bab865b5a60f49` is on `origin/main`; hosted CI stays disabled until GitHub billing is repaired |

The aggregate execution record is
[`results/2026-07-31-railway-simplified-cutover.md`](../results/2026-07-31-railway-simplified-cutover.md).

## Primary sources

- [Railway Dockerfiles](https://docs.railway.com/builds/dockerfiles)
- [Railway databases](https://docs.railway.com/databases)
- [Railway storage buckets](https://docs.railway.com/storage-buckets)
- [Railway private networking](https://docs.railway.com/guides/private-networking)
- [Railway third-party observability](https://docs.railway.com/guides/third-party-observability)
- [AWS S3 versioning](https://docs.aws.amazon.com/AmazonS3/latest/userguide/Versioning.html)

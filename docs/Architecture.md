# Straylight Architecture

Status: alpha reference architecture

Straylight is an agent-first context and durable-work service. It preserves
source evidence, learned knowledge, live work state, artifacts, and resumable
checkpoints so that a later agent can inspect, continue, verify, and advance
work without replaying an entire transcript or reconstructing a filesystem.

This document records the implementation architecture. The four initial design
documents in the shared vault remain the governing product inputs; changing
their read, write, or dreaming semantics requires an explicit design decision.

## System Shape

```mermaid
flowchart LR
    A[Agent or SPA] -->|Bearer token| M[MCP or HTTP API]
    M --> R[Rust service]
    R --> P[(Postgres and pgvector)]
    R --> O[(Versioned S3 object storage)]
    R --> E[OpenAI embeddings]
    R --> D[OpenAI Responses API]
    W[Background worker] --> P
    W --> O
    W --> D
    S[TypeScript SPA] -->|same-origin /api| R
```

All components run in Docker. Postgres, the object store, and the API are bound
to localhost by default. Only the SPA is deliberately reachable from the
local network.

## Product Boundaries

- Straylight is not a transcript archive, notes application, synchronized
  folder, generic RAG wrapper, or universal knowledge graph.
- Markdown and source-native files are evidence and import formats, not the
  canonical interaction model.
- Retrieval is an aid to reasoning, not a truth boundary. Complete bounded
  materialization and exact source reads remain available.
- Arbitrary code does not execute inside Straylight. `memory.compute` is a
  bounded declarative surface; capable agents can run additional analysis in
  their own sandboxes after read-only retrieval.
- Online capture and offline dreaming are separate authority paths.

## Ownership And Isolation

The alpha is multi-user without a separate tenant abstraction. Every durable
row belongs directly to one internal `user_id`; scope IDs further constrain
credentials and sessions. There is one shared service deployment, one database,
and one object store.

Isolation is enforced at several layers:

1. Bearer credentials resolve to one user, a capability set, and explicit
   scope grants.
2. Each database transaction installs signed user, credential, capability, and
   scope context.
3. Postgres row-level security independently checks the context on every
   protected table.
4. Queries also constrain user, scope, and immutable corpus revision directly.
5. Object keys begin with the owning user ID.
6. Embeddings, caches, jobs, audit events, continuation tokens, and import
   receipts never deduplicate across users.

Read-only credentials retain `open`, `query`, `read`, `compute`, `verify`, and
`status`. Ordinary read/write credentials add corpus mutation, staging,
correction, deletion, checkpoint, and dream capabilities. Credential issuance
and revocation require the separate `credential:manage` owner capability, so a
normal writer cannot mint a more powerful token.

## Durable Model

The logical kernel stays small:

- **Objects** provide stable identity and immutable revisions. Profiles such as
  person, organization, event, arrangement, resource, work item, and artifact
  are composable labels over the same object contract.
- **Claims** carry value, producer, formation method, claim mode, support state,
  authority, canonicality, confidence, valid time, lineage, and direct evidence.
- **Qualified relations** preserve endpoint roles, qualifiers, evidence, and
  revision history. Similarity and aliases never establish identity.
- **Temporal and recurrence specifications** preserve civil time, IANA zones,
  intervals, series rules, stable occurrence identity, and sparse exceptions.
- **Named state machines** keep schedule, participation, attendance,
  notification, booking, payment, allocation, availability, use, execution,
  and validation independent.
- **Sources, evidence, documents, chunks, and assets** retain source-native
  content, hashes, locators, and object-storage lineage.
- **Corpus revisions** are immutable manifests over record versions and
  dispositions.
- **Sessions** pin one credential, authorization scope, policy projection, task
  hash, and corpus revision.
- **Checkpoints** are immutable work-state objects with parent linkage, goals,
  decisions, gaps, gates, next actions, and validated source references.

There is no global lifecycle field. A scheduled event may coexist with
tentative participation, unknown attendance, an unpaid arrangement, and an
unverified work result.

## Read Path

`memory.open` creates a snapshot-pinned session and returns a corpus map,
resolved roots, optional checkpoint and revision delta, bounded complete
materialization when it fits, an initial hybrid evidence set, and the latest
fresh hard-gated learned view for the same immutable revision. Learned items
are labeled `derived_non_authoritative`, retain direct source links, and are
omitted rather than served stale when no candidate exactly matches the pinned
revision. They never replace source evidence or alter absence guarantees.

Open is optimized as the first reasoning packet, not merely as a session
handshake. Its selector gives exact references and titles priority, detects a
sharp relevance drop instead of filling every result slot, and groups multiple
relevant sections from the strongest source before broadening. Ordinary query
keeps the source-diverse ranking used for discovery.

After selection, open groups candidates by source and hydrates sources in rank
order. Up to four sources may be loaded completely under one 32,000-character
source budget; the strongest source may consume the shared budget instead of
being rejected by an arbitrary per-file cap. A source that does not fit keeps
its selected exact sections, ranges, and references. `likely_sufficient`
requires both task-anchor coverage and a complete primary source; it is a
retrieval hint, not proof that every requested output facet is supported.

`memory.query` combines independent exact, structured, PostgreSQL FTS,
pgvector semantic, temporal, and relation lanes. Reciprocal-rank fusion merges
candidates while preserving lane scores and `why_selected`. Authority,
canonicality, freshness, contradiction, and valid time remain visible and are
not collapsed into one truth score.

`memory.read` resolves exact typed references and source-native views,
including current state, full content, ranges, outline, neighbors, relations,
history, diffs, and materialized scope.

`memory.compute` performs bounded filters, joins, groups, aggregations,
timelines, diffs, state history, identity resolution, recurrence expansion,
graph traversal, unit-aware arithmetic, proximity checks, and gate rollups.
Every result carries evidence references or an explicit unsupported status.

`memory.verify` accepts claims plus optional evidence or structured coordinates
and classifies them as supported, contradicted, insufficient, superseded, or
temporally ambiguous. It may discover relevant evidence, but it never treats
arbitrary retrieved text as support without a claim match.

The HTTP API retains detailed candidates, coverage, rank receipts, and policy
audit data. Agent adapters expose a compact reasoning view by default: they
remove corpus inventory samples, duplicated envelope fields and candidate
aliases, and ranking mechanics while preserving evidence text, exact paths and
references needed to reopen excerpts, authority, currentness, checkpoints,
deltas, learned context, gaps, conflicts, and policy receipts. Agents start
with open, query only unresolved gaps, and batch exact path, reference, or
range reads when several sources are needed. The transport carries up to twelve hydrated
source entries under a 32,000-character source-text budget, marks overflow
sources as pointers, and never asks an agent to reread a complete source. It
records source-text, metadata, complete-source, pointer, and sufficiency metrics
for evaluation. MCP emits one textual JSON representation by default so the
same payload is not duplicated as `structuredContent`.

## Usage Telemetry

Agent read operations append content-free access telemetry after policy
projection. This does not change any `memory.open`, `memory.query`,
`memory.read`, `memory.compute`, or `memory.verify` request or response field,
and telemetry does not affect retrieval rank, authority, dreaming, retention,
or corpus state.

The post-policy collector inspects only reasoning-bearing fields:

- open evidence, hydrated sources, bounded materialization, checkpoint and
  revision deltas, and learned context
- query results
- successful or partial exact reads
- compute outputs and their evidence
- verification claims and evidence

Corpus-map inventory samples, projection metadata, failed exact-read targets,
write receipts, stages, audit browsing, and control-plane reads do not count as
reasoning use. Every projected response writes at most one immutable event per
distinct visible record, with a reference-occurrence count for diagnostics.

## Production Observability

The API and worker emit bounded, content-free DogStatsD metrics through a
fail-open in-process exporter. Counters are aggregated before transmission and
latency, size, token, candidate, and queue-age histograms are sent as Datadog
distributions so percentiles remain meaningful across replicas.

The metric surface covers HTTP traffic and structured failures, authentication
and capability denial, database transactions and pool pressure, object
storage, embedding and model dependencies, retrieval lanes and coverage,
exact reads, deterministic compute, verification, writes, capture, policy
projection, usage tracking, dreaming, queue health, deletion propagation, and
worker liveness. All series carry unified `env`, `service`, and `version` tags
plus a bounded `component` tag.

Identifiers and content never become metric tags. User, credential, session,
scope, record, source, path, query, title, request ID, model output, and error
message detail remains in audited records or structured logs. The Datadog
Agent is an optional deployment dependency: telemetry failure is visible in
logs and exporter self-telemetry but cannot fail a memory operation.
Events retain user, scope, credential, session, pinned revision, projection
receipt, operation, record ID, and timestamp, but no source text, task text, or
query text.

`GET /v1/usage` rolls chunk, evidence, document, claim, object, relation, and
asset access up to the source episode that supported it. A source use is one
source in one projected response, even when several records from the source
were present. The response includes active, used, and never-used source counts,
operation totals, and bounded most-used, least-used, and least-recently-used
lists. Never-used sources are derived by left joining telemetry against the
active corpus, so absence of an event remains visible.

Telemetry is append-only, user- and scope-isolated with forced RLS, and
queryable by authorized read credentials. Read-only credentials still cannot
mutate corpus or workspace state; the trusted service writes telemetry in the
same way it already writes projection and audit receipts. Telemetry failure is
logged after the projection transaction and cannot fail or alter the read that
produced it.

## Write Path

`memory.capture` is the ordinary source-bearing write surface. It accepts one
source body or an existing source reference, scope, optional roots and intent,
an idempotency key, and automatic or draft-only mode. A bounded OpenAI
structured extraction compiles that input into a complete `memory.save`
request. The compiler adds the source episode, exact-span evidence, authority
dimensions, and optimistic base revision, repairs only mechanical schema
mistakes, and runs deterministic state, identity, completion, confidence, and
source-integrity checks. Low-risk validated captures commit once through
`memory.save`; consequential ambiguity returns an inspectable draft without a
corpus mutation. Existing-source capture must prove that the supplied text is
present in, or exactly hashes to, that source.

`memory.save` is the canonical atomic write operation. A save requires an
authenticated user, scope, source episode, policy revision, idempotency key,
and immutable base corpus revision. It validates all items before a transaction,
locks mutable heads deterministically, writes evidence and revisions, advances
the corpus manifest once, appends audit receipts, and queues derivative work.

The operation distinguishes create, revise, supersede, retract, relate, and
tombstone. It does not expose a generic upsert. Proposals, attempted actions,
file presence, and invitations cannot silently become completion or validation.

`memory.stage` places files or archives in an expiring inspection workspace.
Archive members are inventoried individually with traversal, symlink, expansion,
ratio, and size limits. A stage exposes a read-your-writes mini-corpus through
the normal read/query/compute/verify API. Promotion selects explicit entries and
commits a replay-safe import receipt; archives are never indexed as opaque
containers.

Checkpoint convenience calls map to the canonical checkpoint save contract and
do not define a second persistence model.

## Dreaming

Phase 0 dreaming is shadow-only. A job pins an active revision, region, policy,
model, prompt, schema, budget, and expected query families. Deterministic
maintenance and optional deep consolidation can create only source-bearing
candidate views, aliases, soft links, clusters, flags, and review-required
hypotheses.

The worker observes committed corpus revisions and automatically schedules a
debounced refresh after relevant change or inactivity. A successful candidate
must pass every hard gate and its paired retrieval evaluation before
`memory.open` can include its safe, non-review-required derived items. Inclusion
is automatic only while the candidate is fresh for the exact pinned revision;
quarantined, rejected, failed, stale, or review-required material is excluded.

The worker evaluates active and candidate retrieval behavior, policy and
lineage invariants, transition safety, and source preservation. The SPA exposes
the candidate manifest, findings, gates, evaluation, model usage, review
history, and audit trail. Accepting a Phase 0 review records learning but never
mutates the active corpus. Promotion and rollback become meaningful only in a
later phase with a separately approved contract.

## Deletion

Tombstoning is an explicit destructive workflow, not a memory that says
"forget this." The active corpus immediately marks the target tombstoned and
queues propagation. The worker removes affected embeddings, lexical content,
caches, derived surfaces, and unreferenced object blobs; records each surface
individually; and completes only when required targets are removed or retained
for a concrete policy reason. Minimal content-free tombstone and audit records
remain for stale-reference invalidation and proof of propagation.

Controlled content redaction is available only to the administrative worker
while processing a real deletion job. Application database roles cannot enable
the bypass themselves.

## Storage And Services

| Component | Responsibility |
| --- | --- |
| Rust API | contracts, authorization, transactions, retrieval, control plane |
| Rust worker | embeddings, dream jobs, deletion propagation, background repair |
| PostgreSQL 17 | canonical records, immutable revisions, RLS, FTS, jobs, audit; built-in `C.UTF-8` collation and page checksums |
| pgvector | user-filtered 1536-dimensional semantic embeddings |
| Versioned S3 store | user-scoped source and artifact blobs; qualified for conditional create, metadata, versions, delete markers, and exact purge |
| OpenAI | `text-embedding-3-small`, structured online capture, and bounded Phase 0 consolidation |
| TypeScript SPA | capture, exploration, work, dreaming, audit, settings |
| TypeScript MCP | typed open-first agent operations and compact reasoning views over HTTP |

## Failure Semantics

- Capability and scope failures are explicit and do not widen access.
- A session never silently advances to a newer corpus revision.
- Unsupported filters or compute semantics fail explicitly rather than running
  a broader query.
- Partial and degraded responses identify unsearched partitions and index lag.
- `no_result` is never evidence of absence unless a maintained-complete
  collection proves the searched boundary.
- Idempotent replays return the original receipt; conflicting payloads return a
  stable conflict.
- Capture model or validation failure returns a source-preserving draft and
  never falls through to an unvalidated write.
- Background work retries with bounded attempts and records terminal failures.

## Deployment Boundary

The alpha deployment uses one multi-user service, one PostgreSQL/pgvector
database, and one S3-compatible versioned object store. Production Compose
consumes prebuilt candidate images, exposes only the TLS edge, uses file-backed
secrets, forbids development bootstrap credentials, and provisions owners
through a one-shot database-operator command. Quotas, request limiting,
complete export, account deletion, coordinated backup/restore, release
fingerprinting, and bounded production metrics are implemented.

Final hostname and brand, deployment host and exposure, object-store product,
off-host backup destination and key custody, alert recipients, alpha cohort,
token delivery, spend limits, policy wording, and go/no-go remain owner
decisions. They may not weaken the user, scope, revision, evidence, capability,
metric-privacy, reasoning-quality, or token-efficiency contracts above.

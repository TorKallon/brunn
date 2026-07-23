# Straylight Alpha Specification

Status: implementation and acceptance contract

## Objective

A fresh authorized agent must be able to reopen durable context rooted at any
relevant object, inspect complete source evidence, understand current and
historical state, incorporate changed facts or constraints, advance work, and
leave a source-bearing child checkpoint. The same reasoning surface must work
with a strictly read-only credential.

The alpha succeeds only when it matches or exceeds direct local Markdown access
on answer quality and evidence recall while adding durable continuity,
provenance, authority, user isolation, and auditable mutation.

## Identity And Access

- A user is the top-level owner of all records.
- A scope is an authorization boundary and retrieval constraint, not an object
  identity or mandatory project container.
- Credentials are bearer secrets stored only as hashes.
- Tokens are returned once when created and are never recoverable from status
  or list APIs.
- Read-only tokens expose exactly `open`, `query`, `read`, `compute`, `verify`,
  and `status`.
- Read/write tokens add `checkpoint`, `save`, `stage`, `correct`, `delete`, and
  `dream`.
- `memory.capture` requires the existing `save` capability; it cannot be used
  by a read-only token as a side door to persistence.
- Only owner credentials with `credential:manage` may issue or revoke tokens.
- Every API and database operation is same-user and scope constrained.

## References

External API references are typed strings such as `object:<uuid>`,
`claim:<uuid>`, `evidence:<uuid>`, `source:<uuid>`, `document:<uuid>`,
`chunk:<uuid>`, `checkpoint:<uuid>`, `session:<uuid>`, and
`revision:<uuid>`. Compact and hyphenated UUID spellings resolve to the same
identity. Names, labels, aliases, paths, and embeddings are discovery aids and
never identity proof.

## Objects And Profiles

An object has a stable ID, one current immutable revision per corpus snapshot,
optional handles, labels, structured properties, and one or more namespaced
profiles. Initial core profiles cover person, organization, group, place,
event series, event occurrence, arrangement, resource, work item, artifact,
and project-like domain objects without requiring a project parent.

Object revisions preserve prior version, source episode, recorded time, policy,
profiles, and property values. Revision heads advance sequentially with
optimistic preconditions.

## Claims And Evidence

A claim must explicitly provide:

- at least one `about_ref`
- namespaced predicate and value
- `producer_ref`
- formation method
- claim mode
- support state
- authority
- canonicality
- evidence references or claim lineage unless explicitly asserted

Confidence, when present, is between zero and one. State assignments and
transitions are claims whose predicate is a registered state machine and whose
claim mode matches their operation. The service never supplies missing
authority dimensions from permissive defaults.

Evidence retains source episode, locator, exact text or asset, content hash,
observation time, and policy. Claims, evidence, objects, and relations remain
separately addressable.

## Relations And Identity

Relations are immutable revisions with namespaced predicate, ordered role
endpoints, qualifiers, valid time, evidence, and source lineage.

`core.possibly_same_as` records a reversible candidate. `core.same_as` requires
an approved identity review that explicitly names the relation, reviewer,
decision, reason, and direct evidence. Approval requires owner-level authority.
Both object IDs and inbound references survive approval and reversal.

## Time, Recurrence, And State

Temporal schema version 1 supports:

- date
- date interval
- local datetime with IANA time zone
- floating datetime without zone
- instant
- instant interval
- local interval with IANA time zone
- due datetime

Recurrence uses RFC 5545 rules, a stable series object, stable occurrence
identity, immutable original time, current and actual time, sparse exclusions
and additions, and split-series linkage for "this and future" changes.
Expansion is bounded by an explicit result limit and optional time window.
Stored moved, cancelled, excluded, and added occurrences override generated
series output without erasing their original recurrence identity.

The initial state registry keeps these dimensions independent:

- event schedule and execution
- participation response and attendance
- notification delivery
- arrangement booking, payment, allocation, and use
- resource availability
- work execution
- gate validation

The latest state head is selected before applying requested state values. An
older matching assignment cannot masquerade as the current state.

## Corpus Revisions And Sessions

A corpus revision is an immutable ordered manifest of record IDs, versions, and
dispositions. One atomic write creates at most one successor revision.

`memory.open` validates task size and hash, chooses an authorized scope, pins
the latest or requested corpus revision, snapshots capabilities and policy, and
returns:

- session and corpus revision references
- scope and root resolution
- corpus map by record kind and profile
- checkpoint and exact revision delta when resuming
- complete bounded materialization when it fits
- initial hybrid evidence
- the latest fresh, hard-gated, non-authoritative learned context for the exact
  pinned revision, or explicit pending/stale/unavailable status
- freshness, coverage, conflicts, gaps, and ambiguities

The initial evidence selector prioritizes exact stable references and titles,
then current temporal evidence, groups relevant sections from the strongest
source, and may return fewer than the maximum when fused relevance has a sharp
drop. This source-coherent policy applies only to `memory.open`; explicit
queries remain source-diverse discovery operations.

Sessions are credential-bound, expiring, and immutable. Refresh creates a new
session over an explicit newer revision and exposes the delta; it never mutates
the old session.

## Query

`memory.query` accepts up to 32 queries. Each query requires text, a structured
filter, or a state filter and has a result limit from 1 to 100.

Supported lanes are exact, structured, lexical, semantic, temporal, and
relations. Supported structured filters are `scope_root`, `type_profile`,
`predicate`, `record_kind`, `authority`, and `canonicality`. Unknown filters
are rejected. State filtering supports current heads with
`valid_at: "latest"`; unsupported historical-time semantics are rejected.

Structured `scope.root_refs` and `where.scope_root` constrain the candidate
region to the roots and explicit graph neighborhood. They may narrow but never
widen the session authorization scope.

Each candidate returns typed reference, source reference and version, content
hash, optional path and heading, authority, canonicality, recorded and valid
time, evidence references, lane scores, and `why_selected`.

The detailed HTTP representation above is the audit view. MCP and evaluation
adapters use a compact reasoning representation by default. It must preserve
candidate content, path, stable reference, source version, authority,
canonicality, recorded and valid time, selection reason, nonempty diagnostics,
checkpoint and revision delta, learned context, and a compact policy receipt.
It may omit corpus inventory samples, duplicate envelope fields, duplicate
single-query aliases, complete per-item coverage, content hashes used only for
ranking integrity, lane scores, and fused scores. Exact source reads and the
HTTP audit representation remain available when those omitted fields are the
subject of the task.

## Read

`memory.read` batches 1 to 64 exact requests. Views include current state,
structured, outline, full, range, neighbors, relationships, history, diff,
last known good, and materialized scope. A request returns complete, partial,
unsupported, or failed independently, with bounded truncation and a signed
continuation token when resumable.

Source content is read from MinIO by an authorized user-scoped key. A caller
cannot substitute an arbitrary object key.

## Compute

`memory.compute` is snapshot-pinned and declarative. It accepts bounded steps
and row/token budgets. Initial operators include filtering, sorting, joining,
grouping, aggregation, comparison, diff, timeline, state history, identity
resolution, recurrence expansion, graph traversal, applicability comparison,
unit-aware arithmetic, frame-aware proximity, and gate rollup.

Cross-unit or cross-frame operations require an explicit compatible unit or a
versioned evidenced transform. Unsupported operation shapes return
`unsupported`; they never guess.

## Verify

`memory.verify` accepts 1 to 32 claims. A claim may provide explicit evidence
references and structured `about_ref`, predicate, value, or coverage reference.
The service resolves exact evidence or performs bounded discovery and returns:

- supported
- contradicted
- insufficient evidence
- superseded
- temporally ambiguous

Evidence passages include source, source version, content hash, locator, exact
text when authorized, support kind, and recorded time. Structural checks cover
contradictions, superseded sources, unsupported claims, temporal ambiguity,
recurrence/occurrence loss, and requested coverage. Lexical overlap alone is
not enough to support an unrelated assertion.

## Capture

`memory.capture` accepts source content plus source metadata or an existing
typed source reference, scope, optional roots and intent, optional base corpus
revision, optional idempotency key, and `auto` or `draft` mode. New source
content is compiled atomically into source, exact-span evidence, and typed
domain items. Existing-source content must be present in that source or match
its full content hash.

Structured extraction may propose objects, claims, qualified relations,
state assignments or transitions, temporal and recurrence specs, and
checkpoints. The server owns provenance and authority normalization. It rejects
unsupported identity equivalence, ambiguous targets or actions, unsupported
quotes, confidence below the automatic threshold, cross-state implication,
and attempts presented as completed work. Mechanical profile aliases and
formation-versus-claim-mode mistakes may be normalized without changing the
underlying assertion.

An `auto` request commits only when the model selects commit and every
deterministic check plus canonical `memory.save` validation passes. Otherwise
it returns `needs_review` with the full canonical draft and issues, without
advancing the corpus. `draft` always suppresses commit. Durable no-op input
returns `no_op`. Exact idempotent replay returns the original commit; reusing a
caller key for different content conflicts.

## Canonical Save

`memory.save` requires intent, scope, source references, one or more typed
items, and an idempotency key. Optional base revision and expected object
versions enforce optimistic concurrency.

Valid actions are create, revise, supersede, retract, relate, and tombstone.
Valid kinds are object, claim, evidence, relation, state assignment, state
transition, temporal spec, recurrence spec, checkpoint, source, policy, asset,
identity review, and system-generated import receipt.

All validation happens before commit. The write transaction records the
operation, evidence, immutable domain rows, corpus successor, item receipts,
index state, and audit event. Replaying an idempotency key with the same request
returns the first receipt; a different request conflicts.

## Stage And Promotion

`memory.stage` accepts bounded multipart files and archives. It rejects unsafe
paths, symlinks, duplicate archive paths, decompression bombs, oversized
members, and unreadable entries. Each readable archive member receives its own
inventory entry and blob.

A ready stage acts as a temporary corpus for query, read, compute, and verify.
Promotion names selected entries and a stable import identity. It creates
source/document/chunk records, deduplicates exact content without merging
identity, advances one corpus revision, records every entry disposition, and
returns a durable import receipt. Replays converge.

## Checkpoints

A checkpoint records session, parent checkpoint, corpus revision, ordered
goals, state references, decisions, gaps, acceptance gates, next actions, and
source references. Every referenced record must exist in the pinned corpus and
must not be tombstoned. A child checkpoint preserves exact parent and source
lineage while incorporating revision deltas.

## Dreaming

Dream jobs require scope, trigger, job type, budget, and idempotency key. Jobs
are region-locked, bounded, retryable, and snapshot-pinned. Phase 0 outputs are
source-bearing shadow candidates only. Model input is bounded, uses
`store: false`, and sends a stable privacy-preserving safety identifier.

The verifier must check source integrity, policy boundaries, identity safety,
state separation, contradiction preservation, manifest immutability, and
active-versus-candidate retrieval behavior. Hard-gate failure prevents a
reviewable result. Human accept, reject, or quarantine decisions are audited;
none can mutate the active revision in Phase 0.

The scheduler observes active-manifest changes and automatically enqueues a
bounded, debounced refresh after a dirty threshold or inactivity window. A
candidate whose hard gates and paired retrieval evaluation pass remains a
shadow revision. `memory.open` automatically returns its safe,
non-review-required derived items only when its base revision exactly matches
the session revision. Every returned item includes candidate and dream lineage,
direct source references, model/evaluation receipts, a token estimate, and an
explicit `derived_non_authoritative` boundary. Stale, quarantined, rejected,
failed, and review-required material is never injected.

## Deletion

Tombstone immediately removes a target from the active manifest and creates a
deletion job. Cancellation of an event is a schedule-state change, never
deletion. The worker propagates removal through content-bearing rows, chunks,
lexical search, embeddings, caches, derived views, exports/replicas when
configured, and unreferenced assets. Per-surface receipts are required. A job
cannot report completion while required cleanup is pending or failed.

## Response Contract

Reasoning responses use a common envelope with request ID, session, corpus
revision, status, freshness, coverage, data, conflicts, gaps, ambiguities, and
truncation. Status values distinguish complete, partial, ambiguous, stale,
inconsistent, degraded, committed, no-op, accepted processing, needs review,
conflict, and policy rejection.

Fields carried by the common envelope are not repeated at the top level of
`data`. Policy projection occurs before this transport de-duplication, so the
audit receipt still describes the complete projected response.

Coverage names searched and unsearched partitions. Missing results are not
absence proof unless `absence_safe` is true for a maintained-complete set.

## Acceptance Gates

The alpha is releasable for inspection when all of the following hold:

1. Fresh and repeat migrations succeed and RLS/direct credential-control probes
   deny cross-user and non-manager access.
2. Rust, SPA, MCP, Python harness, migration, live API, and browser tests pass.
3. Read-only denial is exercised against every mutation family at the service
   boundary.
4. Automatic capture, staging, archive-member promotion, checkpoint resume,
   automatic learned-context inclusion, deep shadow dreaming, and deletion
   propagation pass live tests.
5. The native service matches or beats the unchanged local Markdown baseline
   on every active card in the main, Rupture Ops, personal coordination, and
   checkpoint-transition suites. Retired cards remain reproducible historical
   fixtures but do not count toward the current product gate.
6. Failures are reported honestly; no path claims complete coverage, policy
   application, validation, or cleanup that did not occur.

## Deferred Public-Service Decisions

Identity provider, account recovery, quotas, abuse controls, collaborative
ownership, billing, production backup and restore objectives, final retention
periods, and public descriptor are outside the local alpha. They must preserve
the contracts above.

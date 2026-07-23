---
title: "Portable Personal Context Layer"
status: "concept"
project: "Straylight"
created_at: "2026-07-10"
updated_at: "2026-07-10"
source_type: "concept-brief"
tags: ["straylight", "personal-context", "memory", "agents", "semantic-search", "portability"]
---

Related: [[Straylight]]

Source: converted from [[Records/Retained PDFs/Straylight/Portable Personal Context Layer - 2026-07-10.pdf|Portable Personal Context Layer PDF]], pages 1-9.

# Portable Personal Context Layer
## One-Sentence Thesis
A user-owned, portable memory substrate that lets agents across Codex cloud
and local environments, personal laptop agents, OpenClaw/Hermes-style
systems, and Claude Code resume work with durable context while allowing each
person to choose between a unified vault and physically separate trust
domains.
## The Problem
- Useful context is fragmented across devices, agent products, threads, files,
and runtime environments.
- The current Work vault substantially reduces rediscovery, but it is
laptop-centered and depends on each agent knowing how to retrieve and promote
information correctly.
- Agents still suffer a recurring Memento effect: unless context is explicitly
rehydrated or saved, each new session starts too close to zero.
- Saving every transcript or document is not the answer. A full knowledge dump
creates retrieval noise, stale duplication, and privacy risk.
- Human-readable docs are valuable for inspection, but the primary job is
machine continuity: preserving enough context, work state, research, and
learned patterns for the next agent to continue intelligently.
## Product Idea
Create a common memory protocol and product that can operate across one or
more independent vault instances. Some people may use one instance with
multiple scopes; others, including Rourke, need physically separate personal
and work instances. The service would:
- capture durable memories explicitly and, under a clear policy, implicitly
- serve the canonical personal memory directly from a cloud service rather
than making any laptop or dev box the primary store
- use optional local caches only as disposable performance accelerators
- publish one-way full or scoped replicas into physically separate consumer
environments when required
- expose a snapshot-pinned Memory Workspace API through which agents can
iteratively query, read, compute over, and verify evidence
- support complete project materialization when the relevant corpus fits and
progressive exact, lexical, semantic, temporal, and relational retrieval when
it does not

- use context packs only as optional starting hints or derived views, never as
the sole path to the corpus
- persist normal explicit and implicit saves through one evidence-backed
'memory.save' call, using 'memory.stage' only when uploaded material requires
model inspection
- separate fast online capture from scheduled or event-triggered offline
dreaming that consolidates memory across sessions
- preserve provenance, freshness, conflicts, corrections, and deletion
- expose the same protocol and memory model through a Codex plugin, an MCP/CLI
or API surface, a Claude Code integration, and adapters for personal-agent
systems such as OpenClaw or Hermes
The important distinction is that this is not primarily a notes app, a wiki
for humans, or necessarily one central database. It is an agent memory and
context layer with human-readable views and user-chosen trust boundaries.
## What The System Should Remember
- durable user preferences and instructions
- project and workstream state that will matter in a later session
- decisions, owners, commitments, blockers, and corrections
- reusable research findings and pointers to source artifacts
- recurring working patterns, lessons, and successful methods
- relationship or people context, when appropriate to the memory scope
- artifact state: what exists, where it lives, what version is canonical, and
what remains unresolved
## What It Should Not Remember By Default
- every chat turn or raw transcript
- transient implementation chatter or routine status updates
- duplicate summaries of information that already has a canonical
representation
- unsourced speculation presented as fact
- secrets, sensitive personal context, or work-confidential context outside an
explicitly permitted scope
- stale facts without a date, provenance, or supersession path
## Core Design Principles
1. **Memory, not document accumulation.** Store durable meaning and resumable
state, not an indiscriminate archive.
2. **Implicit capture with a promotion policy.** Agents should be able to save
useful context without being asked every time, but only when meaning changes
and within a clear scope.
3. **Explicit control remains available.** 'Remember', 'correct', 'forget',
'do not save', and 'show me what you used' should be first-class operations.
4. **Portable and user-owned.** The memory should outlive any single agent
vendor, device, runtime, or deployment topology and support open export.
5. **User-chosen trust topology.** Namespaces may be sufficient for some
people, but the product must also support physically separate personal,
employer, project, and team instances. One interface must not imply one
database or one security boundary.

6. **Inspectable and reversible.** Every injected memory should have
provenance, date, confidence or evidence state, and a correction/deletion
path.
7. **Reasoning-first retrieval.** Give the model an inspectable, iterative
evidence workspace with exact source and full-artifact access. Canonical
summaries and embeddings are useful retrieval aids, not the boundary of what
the model may inspect or the source of truth.
8. **Freshness-aware.** The system must distinguish stable preferences from
volatile owners, dates, plans, and commitments that need live verification.
9. **Fast capture, slow consolidation.** Make live persistence cheap,
evidence-backed, and append-first. Run semantic deduplication, cross-session
rationalization, abstraction, and active-set compaction in a separate
dreaming process that preserves source history.
## Personal Cloud Authority Model
For Rourke, the personal system should be cloud-native and cloud-authoritative:
- all canonical memories, artifacts, project state, versions, and indexes live
in the personal cloud service
- personal agents and clients read and write that service directly whenever
network access is available
- local caches may hold encrypted objects, chunks, or indexes to reduce
latency, but they are disposable and never become an authority
- deleting a cache or losing a laptop must not lose state or require recovery
from another device
- client caches use object versions, hashes, and expiry to invalidate stale
data; there is no peer-to-peer or multi-master reconciliation
- the personal service exposes the authoritative Memory Workspace API and its
exact, lexical, semantic, temporal, relational, source-read, compute, and
verification capabilities to personal clients
This deliberately moves away from a synchronized-folder model. There is one
cloud source of truth and many direct clients. Offline behavior, if offered,
should be a bounded cache or explicit queued draft rather than a second
authoritative store that later needs general conflict resolution.
## Rourke's Work And Personal Trust Model
Rourke would never place OpenAI work memory and personal memory in the same
instance. His required topology is:
```text
Cloud-authoritative personal memory service
-> signed, encrypted snapshots and deltas
-> dedicated one-way distribution outbox
-> read-only work importer
-> complete work-local personal replica and indexes
-> Work Codex
```

The governing rule is **personal context may flow into the work environment;
work context must never flow into the personal system**.

A normal read-only personal API is not sufficient for the strongest version of
that promise. It prevents Work Codex from mutating personal records, but a
work-generated search string, embedding, request body, error, telemetry
event, or access log would still send work-derived information toward the
personal service. Prompt injection could deliberately encode confidential
work data into such a request.
For Rourke's deployment, use application-layer one-way replication:
- the personal cloud publishes a complete, versioned export of its memories,
artifacts, project state, chunks, and deletion tombstones; the product may
support narrower exports for other users
- the personal publisher can write only to a neutral outbox and cannot read
work state or work-generated acknowledgements
- the work importer can read only its relay inbox and has no route,
credential, or write capability for the personal service
- the work importer maintains a complete local replica and builds its lexical
and vector indexes locally; no work query or embedding is sent to the
personal system
- imported personal objects remain visibly tagged `personal-import`, retain
personal provenance, and are non-exportable and non-promotable inside work
memory by default
- any transfer of work-generated output back into personal memory requires a
separate, explicit, human-reviewed declassification step
This provides a strong software-enforced one-way boundary. A literal physical
zero-return-path guarantee would require a hardware data diode or manual
signed export, so the product should describe the assurance level precisely
rather than treating `read-only` as synonymous with `one-way`.
Personal data imported into a work environment creates a different risk: it
may enter employer-controlled prompts, transcripts, caches, retention
systems, administrator access, or legal discovery. Full-replica mode
therefore requires explicit consent, encryption at rest, no automatic
repersistence into ordinary work memory, no re-export, and a clear warning
about the destination environment. The product should also support narrower
project/field exports and ephemeral access for people who do not want
Rourke's full-replica policy.
## Proposed Memory Architecture
1. **Source/event layer:** references to conversations, artifacts, tasks,
commits, notes, imported evidence, and immutable uploaded source packs.
2. **Online capture/write layer:** one-call explicit and implicit persistence
with evidence, task checkpoints, exact deduplication, immutable versions, and
bounded policy enforcement.
3. **Typed memory layer:** compact memory objects such as preference,
decision, project state, commitment, pattern, research finding, artifact,
relationship, or correction.
4. **Artifact, asset, and project-state layer:** versioned itineraries,
reports, source files, reservations, records, costs, opaque blobs, statuses,
and canonical pointers that should not be flattened into isolated facts.

5. **Dreaming and compiled-knowledge layer:** scheduled or event-triggered
cross-session consolidation that produces derived summaries, relationships,
duplicate clusters, retrieval improvements, and proposed canonical changes
while preserving source episodes.
6. **Retrieval layer:** snapshot-pinned, iterative exact, lexical, semantic,
temporal, relational, and hierarchical retrieval with full-artifact
escalation.
7. **Reasoning workspace:** exposes corpus maps, complete project
materialization, direct source reads, programmable comparison and
aggregation, and claim verification. Context packs are optional derived views
rather than the primary retrieval boundary.
8. **Cloud service, replication, and adapter layer:** authoritative personal
API and search, disposable client caches, signed snapshot/delta export feeds,
Codex plugin, local CLI/MCP/API, Claude Code integration, and personal-agent
connectors.
9. **Human control layer:** review, search, edit, pin, correct, forget,
export, share-to-consumer policy, and audit views.
Markdown should remain an important inspectable projection and export format.
It does not have to be the only or primary storage model if structured
objects, relationships, synchronization, and conflict handling require a
database-backed canonical layer.
## Retrieval Requirements
The canonical initial retrieval contract is [[Retrieval API - Initial Design]]. Its governing requirements are:
- optimize first for model reasoning quality, complete evidence chains,
recovery from bad retrieval, temporal accuracy, contradiction detection, and
full-artifact comprehension
- open a stateful, read-only session pinned to one corpus revision
- expose `memory.open`, `memory.query`, `memory.read`, `memory.compute`, and
`memory.verify`
- reveal a navigable corpus map and explicit coverage rather than returning
only opaque top-k results
- materialize a complete relevant project when it fits comfortably
- otherwise interleave exact, structured, lexical, semantic, temporal,
relational, and hierarchical retrieval with model reasoning
- preserve source-native artifacts, document order, headings, tables, stable
locators, immutable versions, and neighboring context
- separate relevance from authority, canonicality, evidence state, and
freshness
- detect contradiction and supersession instead of silently blending
incompatible memories
- support read-only programmable scans, joins, filtering, diffs, timelines,
aggregation, and evidence-preserving reduction
- verify consequential claims against newer, contradictory, superseded, or
missing evidence
- treat context packs as optional seed material or derived output, not the
primary interface or proof of complete coverage
- distinguish authenticated governing instructions from ordinary retrieved
evidence

- make partial coverage, degraded indexes, ambiguity, staleness,
inconsistency, and truncation explicit and resumable
## Capture And Update Loop
The canonical initial write and consolidation contract is [[Write API and Dreaming - Initial Design]]. The detailed offline consolidation
architecture is [[Dreaming Architecture and Plan - Initial Design]]. Their governing model is:
1. Run a lightweight formation pass after each turn, but do not require every
turn to create durable semantic memory.
2. Use 'memory.save' as the ordinary one-call persistence interface for
memories, sources, artifacts, relationships, checkpoints, corrections, and
attachments.
3. Use 'memory.stage' only when files, folders, archives, attachments, or
content packs need to become a temporary corpus for model inspection before
persistence.
4. Preserve exact evidence and explicit corrections online with minimal
latency, exact deduplication, immutable versions, and atomic commit.
5. Refresh task-scoped resumable checkpoints independently from promotion into
durable semantic memory.
6. Queue sensitive, inferred, broad, cross-scope, destructive, or ambiguous
changes for review rather than interrupting ordinary low-risk saves.
7. Run a separate scheduled or event-triggered dreaming process for semantic
deduplication, conflict analysis, relationship discovery, abstraction,
compiled summaries, stale-state review, and retrieval compaction.
8. Keep source episodes and provenance inspectable; dreaming generates derived
views and proposals rather than silently rewriting history.
## Threat Model
- employer-confidential context leaking into a personal service or another
employer/project scope
- a supposedly read-only API leaking work prompts, embeddings, request
metadata, logs, errors, timing, or telemetry to the personal service
- personal memory silently appearing in a shared or work context
- prompt injection or source poisoning being promoted into trusted memory
- imported personal content being interpreted as agent instructions or tool
authorization
- origin laundering, where 'personal-import' context is silently promoted into
ordinary work memory or re-exported elsewhere
- inferred traits or relationship judgments hardening into apparent fact
through repetition
- stale context continuing to rank because it is semantically similar
- incomplete deletion that removes the visible memory but leaves chunks,
embeddings, caches, or derived summaries behind
- implicit capture feeling like surveillance because users cannot see or
control what was retained
- uneven host capabilities making one integration appear automatic while
another only supports explicit memory tools
## Strongest Product Wedge

The differentiated idea is not "a vector database for chat history." Vector
search is necessary but increasingly commodity infrastructure.
The stronger wedge is a portable memory control plane for a person's agents:
- one user-owned context substrate across otherwise disconnected agent
ecosystems
- support for either a unified personal fabric or multiple physically separate
vault instances under the same protocol
- selective implicit capture rather than transcript hoarding
- governed, directional context movement rather than one undifferentiated
global brain
- provenance, corrections, freshness, and conflict resolution
- open adapters and exports that reduce vendor lock-in
## MVP Recommendation
Start narrower than the eventual platform:
- one cloud-authoritative personal service accessed directly by personal
clients, with optional disposable encrypted caches
- an open typed-memory schema plus source pointers
- five initial memory classes: preference/instruction, decision, project
state, reusable pattern, and research finding, plus first-class artifact
objects
- Codex local/cloud as the primary integration, including a one-way
personal-to-work import path
- explicit remember/correct/forget plus end-of-turn implicit capture and
task-scoped resumable checkpoints
- one-call 'memory.save' for explicit and implicit persistence, plus
'memory.stage' for inspectable content packs and large source material
- a separate offline dreaming process for cross-session consolidation,
conflict surfacing, relationship discovery, and active-memory compaction
- the reasoning-first Memory Workspace API defined in [[Retrieval API - Initial Design]], including direct, iterative, programmable,
full-artifact, and verification paths
- a simple review/audit view and Markdown/JSON export
- a complete one-way work replica built from signed and encrypted
snapshots/deltas, with separate identities, keys, endpoints, and control
planes
- hard instance boundaries between work and personal memory, with full-replica
and narrower project/field export policies
- sequence numbers, hashes, replay and rollback protection, atomic imports,
stale/partial-import warnings, and deletion tombstones
- verifiable deletion from the canonical store, retrieval index, embedding
cache, and derived summaries
Do not begin with a universal knowledge graph, autonomous ingestion of every
source, or integrations for every agent. Prove that the system measurably
reduces rediscovery and improves continuation quality first.
## Evaluation Questions
- Which vault surfaces are repeatedly read and materially improve an answer or
task?

- Which implicit writes are later reused, and which become noise or stale
duplication?
- How often does a task rediscover context that already existed but was
difficult to retrieve?
- Which memory types create the most continuation value: preferences, project
state, decisions, research, people context, or artifact pointers?
- Does semantic search find useful prior context that indexes and exact search
miss?
- What retrievals are ignored, wrong, stale, or inappropriately scoped?
- How much context is needed to resume a task without flooding the model?
- Where do work and personal boundaries need to be technically enforced rather
than left to agent judgment?
- When personal context is consumed at work, what can be ephemeral, what may
be cached, and what must never be promoted into work memory?
## Vacation Planning Acceptance Test
Vacation planning is a concrete end-to-end test for whether this is a real
memory product rather than a fact store.
The personal vault should represent a 'Trip' project containing:
- stable constraints: possible dates, budget limits, traveler preferences, and
personal limitations
- destination research, comparisons, source links, and open questions
- sample itineraries and other versioned artifacts
- decisions and rejected options, with reasons
- a booking ledger covering flights, lodging, activities, deposits,
cancellation terms, costs, and 'candidate', 'reserved', 'booked', 'canceled',
or 'not booked' status
- a compiled current-state summary that can be regenerated from the underlying
memories and artifacts
Personal agents access the authoritative cloud 'Trip' project directly. Under
Rourke's full-replica policy, the trip is automatically included in the
personal cloud's signed export manifest and encrypted snapshot/delta stream.
The work importer verifies signatures, hashes, schema, sequence, and
tombstones, then updates the complete work-local replica and its indexes.
Other users may choose project- or field-scoped exports instead.
When Rourke asks Work Codex to continue planning, it opens a snapshot-pinned
reasoning workspace over only the local 'personal-import' replica. Work Codex
can materialize the full 'Trip' project when it fits or iteratively query,
read, compute over, and verify the relevant constraints, research, itinerary
versions, costs, and booked-versus-unbooked state. Any initial case file is
only a starting hint; exact state, neighboring evidence, relationships,
history, and complete artifacts remain directly inspectable.
Acceptance criteria:
1. Rourke does not have to restate dates, budget, preferences, prior research,
or booking state.
2. Retrieved claims link back to the canonical personal memory or artifact and
preserve version and freshness.

3. Booked, reserved, rejected, and still-open options remain distinguishable.
4. No work query, prompt, embedding, output, telemetry, or derived state
reaches the personal instance or its logs.
5. The work replica contains every canonical personal memory, artifact, and
deletion state included by Rourke's full-replica policy.
6. Imported personal data is not silently promoted into ordinary work memory
or re-exported.
7. Work Codex cannot modify the trip. Any work-generated recommendation must
be moved back through an explicit human-reviewed step.
8. Personal cloud updates, corrections, and deletions propagate in later
signed deltas and invalidate stale local chunks and embeddings.

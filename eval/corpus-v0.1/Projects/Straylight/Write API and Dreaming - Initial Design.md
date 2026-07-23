---
title: "Write API and Dreaming - Initial Design"
status: "locked-initial-design"
project: "Straylight"
created_at: "2026-07-10"
updated_at: "2026-07-10"
decision_owner: "Rourke McNamara"
tags: ["straylight", "personal-context", "memory", "write-api", "capture", "dreaming", "consolidation"]
---

Related: [[Straylight]]

Source: converted from [[Records/Retained PDFs/Straylight/Portable Personal Context Layer - 2026-07-10.pdf|Portable Personal Context Layer PDF]], pages 10-21.

# Write API and Dreaming - Initial Design
## Decision
The initial write design uses two timescales:
1. A **fast online capture plane** that preserves newly learned information
quickly, cheaply, and with exact provenance.
2. A **slow offline dreaming plane** that periodically makes the accumulated
corpus more coherent, connected, compact, and useful for reasoning.
The ordinary model-facing write interface is intentionally small:
- 'memory.save' persists one or more memories, sources, artifacts,
relationships, checkpoints, corrections, and attachments in one logical
operation.
- 'memory.stage' makes files, folders, archives, attachments, or content packs
available as a temporary corpus when the model needs to inspect them before
deciding what to persist.
The transactional Memory Formation Workspace remains an internal
implementation, debugging, migration, consolidation, and exceptional
high-impact interface. Models should not routinely perform 'open -> stage ->
review -> commit' for ordinary saves.
This is the canonical initial design for the write and consolidation portions
of the Personal Context Layer.
The detailed offline consolidation architecture, scheduler, region-selection
process, transition verification, promotion gates, rollback model, and phased
rollout are locked in [[Dreaming Architecture and Plan - Initial Design]]. That companion note governs the dreaming plane when this note
is less specific.
## Central Recommendation
**Save new information quickly through one evidence-backed atomic call.
Preserve source episodes and explicit corrections immediately. Use a separate
offline dreaming process to consolidate, relate, rationalize, and compact
memory across time. Require the full staged formation workflow only for bulk,
destructive, ambiguous, or high-impact changes.**

Robust transactions should protect the model rather than become work the model
must perform.
## Why Online Capture And Dreaming Are Separate
Online acquisition and global consolidation require different context,
latency, and authority:
- the online path knows what just happened and can preserve it with high
source fidelity
- the dreamer can inspect related memories across sessions and time, which is
necessary for pattern discovery, semantic deduplication, conflict analysis,
abstraction, and compaction
- forcing broad consolidation after every interaction adds latency and can
irreversibly merge or delete useful distinctions before enough evidence exists
- retaining source episodes allows later consolidation to be corrected,
regenerated, or audited
Research grounding:
- [LightMem](https://arxiv.org/abs/2604.07798) modularizes online writing and
offline long-term consolidation.
- [Auto-Dreamer](https://arxiv.org/abs/2605.20616) decouples fast per-session
acquisition from slow cross-session consolidation.
- [Useful Memories Become Faulty When Continuously Updated by
LLMs](https://arxiv.org/abs/2605.12978) finds that forced continuous
consolidation can damage useful memory and argues for preserving raw episodes
and gating consolidation.
- [Generative Agents](https://arxiv.org/abs/2304.03442) separates observations
from higher-level reflections synthesized over time.
## Responsibility Boundary
| Fast online capture/write | Slow offline dreaming/consolidation |
| --- | --- |
| Runs after a turn or explicit user request | Runs on a clock, threshold, task boundary, import completion, or quality signal |
| One cheap logical save | Potentially expensive multi-hop corpus analysis |
| Preserves exact evidence and provenance | Abstracts across multiple memories and sessions |
| Refreshes task checkpoints | Produces coherent project, topic, and procedural views |
| Persists explicit user statements | Finds semantic duplicate clusters |
| Applies explicit corrections | Surfaces unresolved conflicts and temporal overlaps |
| Performs exact hash and source-identity deduplication | Proposes semantic consolidation and supersession |
| Creates immutable records and versions | Builds relationships and temporal narratives |
| Enforces scope, policy, and obvious conflicts | Compacts the active retrieval surface |
| Makes new material immediately addressable | Improves chunking, indexes, aliases, and summaries |
### Online capture must do

- authenticate the triggering user or policy event
- determine or validate the destination scope
- preserve the exact source span, source pointer, or source artifact needed
for provenance
- record valid/effective time and observation/transaction time
- perform exact hash, source-ID, operation-ID, and idempotency checks
- create immutable object and artifact versions
- apply an explicit user correction immediately when its target is unambiguous
- detect obvious stale-revision or concurrent-write conflicts
- reject unauthorized scopes, secrets, malformed data, and unsupported
sensitivity
- return an exact commit receipt and indexing state
### Online capture must not routinely do
- scan the entire corpus for semantic duplicates
- collapse related memories because their embeddings are similar
- generate global topic summaries
- resolve ambiguous conflicts between independent authorities
- infer broad user preferences from isolated behavior
- physically delete older semantic records merely because they appear stale
- require a multi-step formation workflow for ordinary explicit saves
### Dreaming should do
- semantic duplicate clustering
- cross-session pattern discovery
- repeated-preference and reusable-method consolidation
- contradiction and temporal-overlap analysis
- relationship and entity-link discovery
- compiled project, topic, person, and procedural summaries
- review of provisional or expiring memories
- active/archive surface compaction
- rechunking, reindexing, and retrieval-alias improvement
- retrieval regression tests and evidence-coverage checks
- proposals that one record should merge with, supersede, or be derived from
another
## Dreaming Authority
Dreaming operates over an immutable corpus snapshot through the
reasoning-first retrieval API. It emits evidence-backed, versioned changes
through the same underlying write system.
Dreaming may automatically publish:
- derived summaries labeled 'derived'
- lexical and semantic indexes
- reversible retrieval aliases
- soft relationship edges
- duplicate clusters without destructive merging
- source-availability and freshness flags
- expiration warnings
- reversible active/archive classifications

Dreaming should normally propose rather than silently commit:
- canonical merges
- supersession of user-authored facts or decisions
- selection of a winner between conflicting authorities
- promotion of an inference into a user preference
- global governing instructions
- people or relationship judgments
- cross-scope movement
- deletion or destructive consolidation
The newest record is not automatically the most accurate. Canonicality should
account for authority, evidence, effective time, explicit correction, and
scope.
Exact duplicate bytes may be physically deduplicated. Semantically redundant
records should normally be linked, archived, or superseded while their source
evidence remains inspectable. A coherent topic or project view is a derived
compiled memory, not a rewrite of its source episodes.
## Routine Model-Facing API
| Tool | Purpose |
| --- | --- |
| `memory.save` | Persist one or more memories, sources, artifacts, relationships, checkpoints, corrections, and attachments in one logical operation |
| `memory.stage` | Make files, directories, archives, attachments, or content packs available as a temporary corpus when the model must inspect them before persistence |

Ordinary explicit and implicit saves should take one `memory.save` call.
`memory.stage` is needed only when the model must inspect or select uploaded
material first.
## `memory.save`
### Purpose
Persist a compact batch of source-linked information in one logical operation.
The service performs transaction setup, schema validation, policy checks,
exact deduplication, optimistic concurrency, atomic commit, audit, and index
scheduling internally.
### Example request
```json
{
  "intent": "implicit_turn_end",
  "scope": "project:summer-trip",
  "source_refs": [
    {
      "ref": "conversation:thread-18/turn-92",
      "span": [42, 79],
      "content_hash": "sha256:..."
    }
  ],
  "items": [
    {
      "action": "create",
      "kind": "constraint",
      "content": {
        "source_text": "I cannot travel before August 10.",
        "structured": {
          "earliest_departure": "2026-08-10"
        }
      },
      "epistemic_status": "user_asserted",
      "effective_from": "2026-07-10"
    }
  ],
  "idempotency_key": "thread-18-turn-92-capture"
}
```

### Supported item classes
- memory assertions and corrections
- project and artifact state
- task checkpoints
- source records and source snapshots
- reasoning-bearing artifacts
- relationships
- asset references and attachments
- retractions and deletion requests
### Supported lifecycle actions
- `create`
- `revise`
- `supersede`
- `retract`
- `relate`
- `tombstone`

Generic `upsert` is intentionally avoided because it hides whether the system
created a duplicate, revised a logical object, superseded current state, or
destroyed useful history.
### Required epistemic labels
- `user_asserted`
- `source_asserted`
- `model_extracted`
- `model_inferred`
- `model_synthesis`
- `verified_action`

An explicit request to save a Slack thread or presentation authorizes
persistence. It does not transform statements in that source into
user-authored truth.
### Response statuses
- `committed`
- `no_op`
- `accepted_processing`
- `needs_review`
- `conflict`
- `rejected_by_policy`
### Example response
```json
{
  "status": "committed",
  "corpus_revision": "rev_9191",
  "saved": ["memory:trip-date-constraint:v1"],
  "search_status": {
    "exact": "ready",
    "lexical": "ready",
    "semantic": "pending"
  },
  "receipt": "commit:781"
}
```
Ordinary explicit saves should commit without a redundant second confirmation.
When a proposed effect materially exceeds the user's request, 'needs_review'
returns a compact diff and a confirmation token. The same operation may be
called again with that authenticated confirmation; separate prepare and
commit tools are not required for the routine surface.
## `memory.stage`
### Purpose
Make one or more files, directories, archives, attachments, or content packs
available as a temporary, immutable mini-corpus when the model needs to
inspect or select material before persistence.
### Example request
```json
{
  "inputs": [
    {"local_path": "/path/to/starrupture-research.zip"}
  ],
  "purpose": "Import durable research, current project state, and useful artifacts",
  "preserve_original": true
}
```

### Example response
```json
{
  "stage_ref": "stage:imp_123@rev7",
  "status": "ready",
  "inventory_summary": {
    "files": 118,
    "readable": 109,
    "opaque": 7,
    "quarantined": 2
  },
  "warnings": []
}
```
The existing retrieval API operates over 'stage_ref'. The model can open,
query, read, compute over, and verify the staged mini-corpus before calling
'memory.save' with the selected memories, artifacts, sources, relationships,
and opaque assets.
The adapter handles byte streaming, multipart upload, checksums, safe archive
inventory, resumability, quarantine, and derivative creation. Binary data
must not pass through the model's context as base64.
For a single opaque attachment that does not require inspection, 'memory.save'
may accept the attachment handle directly and stage it internally.
## Implicit End-Of-Turn Capture
Every turn receives a lightweight formation evaluation. This does not mean
every turn creates a durable semantic memory.
The pass has two lanes:
1. **Continuity lane:** refresh the active task checkpoint when there is
meaningful task state: objective, current artifact, last completed step,
blocker or error, pending decision, and next safe action.
2. **Durable-memory lane:** save stable decisions, corrections, preferences,
constraints, verified state changes, canonical artifact versions,
commitments, or reusable methods.
Appropriate automatic commits include:
- directly stated low-sensitivity preferences and constraints
- explicit decisions and corrections
- verified project or artifact state changes
- canonical artifact pointers or versions
- clear commitments, blockers, open questions, and task checkpoints
Candidates that should remain pending or provisional include:
- inferred preferences
- sensitive information
- people or relationship judgments
- broad global instructions

- cross-scope writes
- unexplained conflicts with canonical state
- volatile facts without a validation or expiry rule
Routine chatter, speculation, assistant narration, raw transcripts, unverified
completion claims, and duplicate recaps should produce 'no_op'.
Only the selected turn spans needed as evidence should be retained by default,
plus a stable turn reference and hash. The whole transcript should not become
durable memory merely because capture ran.
## Explicit 'Remember This'
An authenticated user request normally authorizes immediate scoped persistence.
The model should:
1. Preserve the exact user statement as evidence.
2. Extract only what the user actually stated.
3. Search or resolve the relevant existing object when needed.
4. Create, revise, or supersede the appropriate object.
5. Call 'memory.save' once.
6. Report what was saved and what it replaced.
A clear correction should become current immediately rather than waiting for a
dream cycle. History remains available through immutable versions and
explicit supersession.
## Explicit Source Extraction
When asked to read a Slack thread, presentation, document, meeting, or other
source and save its insights, persist three separable layers:
```text
source-native evidence
-> source-linked extracted claims and project state
-> optional derived synthesis
```

The model reads with its existing source tools and then calls 'memory.save'
once with:
- an immutable source record, snapshot, or durable pointer
- the selected insights with exact message, slide, page, section, or block
locators
- relationships from every claim to its evidence
- relevant project-state, artifact, checkpoint, or open-loop updates
- an optional synthesis labeled 'model-synthesis'
Slack captures should preserve thread and message identity, order, authorship,
source edits or versions when available, and completeness status.
Presentation captures should preserve the original asset or version, slide
order, notes, tables and images when text extraction loses meaning, slide
locators, and extraction coverage.

A pointer-only source is labeled 'pointer_only'; future agents must be able to
see that it may disappear or require credentials.
If a newly saved source contradicts canonical state, save the source and
contradiction immediately. Dreaming or explicit review adjudicates the
canonical winner unless the user explicitly made the correction.
## Content-Pack Import
There is no source-specific importer and no opaque `upload_zip_and_summarize`
operation.
The generic flow is:
1. Call 'memory.stage' with a file, folder, archive, or attachment.
2. Open the returned stage through the retrieval API.
3. Inventory, search, read, compare, and inspect the material without
executing it.
4. Assign dispositions to relevant entries.
5. Call `memory.save` once with the import manifest, selected memories,
artifacts, source records, relationships, and asset references.
6. Return a durable completion report covering inspected, committed, linked,
deduplicated, skipped, unreadable, unresolved, and quarantined material.
Useful dispositions include:
- 'derive_memory'
- 'create_artifact'
- 'new_version_of'
- 'attach_to_existing'
- 'retain_as_source'
- 'opaque_asset_only'
- 'duplicate_link'
- 'skip'
- 'unresolved'
The original pack should normally be preserved as a source asset. Foreign
embeddings, indexes, summaries, and model outputs may be retained with
provenance
but are not imported as authority.
Archives are treated as hostile containers. Inventory and extraction enforce
limits on expanded size, file count, compression ratio, nesting, path length,
CPU, and time. Path traversal, symlink escape, special files, archive bombs,
macros, scripts, executables, and database contents are never executed merely
because they were imported.
## Blobs, Assets, And Artifacts
Use three layers:
- **Blob:** immutable internal bytes, content-addressed within one trust
domain.
- **Asset:** stable, user-visible, versioned handle pointing to a blob or
immutable tree manifest.
- **Artifact:** reasoning-bearing project or source object that may reference
one or more asset versions.

Memories and artifacts reference immutable asset versions rather than storage
URLs. Download URLs are minted only at access time.
Example asset reference:
```json
{
  "asset_ref": "asset:starrupture-research-db@v3",
  "role": "supporting_database",
  "reasoning_policy": "metadata_only",
  "execution_policy": "never_execute",
  "integrity": {
    "sha256": "...",
    "size_bytes": 48234496
  }
}
```
Supported reasoning policies:
- 'extract_and_index'
- 'safe_derivatives'
- 'metadata_only'
- 'opaque_no_index'
SQLite databases, archives, tools, binaries, models, and directory bundles may
remain opaque and downloadable. Their metadata, description, provenance,
version, integrity, and relationships remain searchable. Running persisted
code or loading database extensions is a separate capability outside the
memory write API; executable assets default to non-executable.
## Internal Transaction Contract
Although ordinary agents see one logical save, the service internally provides:
- authenticated trigger and scope
- base-revision and object-version preconditions
- transaction and operation idempotency
- immutable source, memory, artifact, and asset versions
- exact provenance and transformation lineage
- exact identity deduplication
- atomic commit by default
- explicit atomic groups for exceptionally large imports
- no silent partial success
- no silent last-write-wins
- immediate exact reads after commit
- explicit lexical and semantic indexing readiness
- durable commit and import receipts
Semantic similarity may suggest a duplicate, contradiction, derivation, or
consolidation candidate. It must never destructively merge records in the
online path.
## Exceptional Formation Workspace

The full internal formation workflow remains available for:
- large or destructive imports
- migrations
- ambiguous canonical changes
- bulk supersession or deletion
- debugging and administrative repair
- dreaming and consolidation jobs
- retrieval regression testing over a staged future corpus
That workflow may pin a base revision, stage a delta, expose a
read-your-writes overlay through the retrieval API, generate a
reasoning-impact report, and commit atomically. It is an exception and
internal capability, not the ordinary save experience.
## Correction, Retraction, And Deletion
- 'revise' creates a new immutable version of the same logical object.
- 'supersede' makes a different object current while preserving lineage to the
prior object.
- 'retract' marks a prior claim as no longer asserted without supplying a
replacement.
- 'tombstone' initiates explicit deletion or forgetting.
- 'contradicts' preserves unresolved incompatible evidence.
Deletion removes information rather than adding a 'please forget' memory. It
must propagate through normalized blocks, lexical indexes, embeddings,
caches, compiled summaries, published snapshots, replicas, and unreferenced
assets. A minimal non-content tombstone may remain where required for
replication and stale-reference invalidation.
## Explicitly Rejected
- requiring `open -> stage -> review -> commit` for ordinary saves
- mandatory staged-overlay testing for every implicit memory
- a raw 'remember(text)' endpoint that discards provenance and structure
- treating every evaluated turn as a durable write
- raw transcript or tool-output accumulation
- generic 'upsert'
- destructive semantic deduplication in the online path
- silent canonical overwrite
- treating external source instructions as user instructions
- allowing the model to self-assign user authority
- direct archive-to-vector-index ingestion
- forced consolidation after every interaction
- deleting source episodes when generating a coherent summary
- waiting for every semantic derivative before acknowledging durable
persistence
## Initial Evaluation
Evaluate online capture and dreaming separately.
### Online capture metrics

- explicit-save success rate
- missed durable-memory rate
- false or noisy auto-save rate
- source-grounding accuracy
- exact duplicate rate
- correction and supersession accuracy
- task-checkpoint recovery
- median and p95 write latency
- percentage of turns resulting in 'no-op'
### Dreaming metrics
- downstream retrieval and answer improvement
- complete evidence-chain recall
- conflict and stale-state detection
- semantic duplication reduction
- active-memory size reduction
- incorrect merge or deletion rate
- preservation of source traceability
- regression rate on a fixed golden query set
The dreamer should not ship with destructive automatic consolidation until it
can improve retrieval and reasoning without losing source-backed information
on representative corpora.
## Deferred Questions
The initial dreaming architecture and process are resolved in [[Dreaming Architecture and Plan - Initial Design]]. Remaining
implementation questions are:
- empirically tuned dirty-score, evidence-volume, recurrence, cooldown, and
region-size thresholds within the locked hybrid scheduler
- whether later versions add task-specialized consolidation programs beyond
the locked initial strong-dreamer, deterministic-worker, independent-verifier
topology
- review UX for canonical merge, deletion, and sensitive inferences
- storage and execution location for staged content
- retention periods for task checkpoints, provisional memories, and abandoned
stages
- adapter-specific support for turn-end hooks
- limits, pricing, and lifecycle for large opaque assets

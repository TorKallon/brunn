---
title: "Retrieval API - Initial Design"
status: "locked-initial-design"
project: "Brunn"
created_at: "2026-07-10"
updated_at: "2026-07-10"
decision_owner: "Rourke McNamara"
tags: ["brunn", "personal-context", "memory", "retrieval", "api", "reasoning", "agents"]
---

Related: [[Brunn]]

Source: converted from [[Records/Retained PDFs/Brunn/Portable Personal Context Layer - 2026-07-10.pdf|Portable Personal Context Layer PDF]], pages 36-48.

# Retrieval API - Initial Design
## Decision
The initial retrieval design is a **stateful, read-only Memory Workspace API**
optimized first for model reasoning quality.
The model opens a session pinned to one corpus revision, inspects an explicit
map of what exists, and iteratively retrieves, reads, compares, computes over,
and verifies evidence. A context pack may be returned as a starting hint or
generated as a derived view, but it is not the primary interface and must never
define the boundary of what the model is allowed to inspect.
This is the canonical initial design for the retrieval portion of the Personal
Context Layer. It supersedes earlier context-pack-first implications in the
concept discussion. Storage, capture, privacy, replication, and commercial
constraints must be reconciled with this interface later; they should not
narrow the reasoning surface before evidence shows that a change improves
reasoning.
## Optimization Priority
Design and evaluation should optimize in this order:
1. Final-answer correctness and completeness
2. Complete evidence-chain recall, including multi-hop evidence
3. Recovery from a bad initial retrieval
4. Temporal accuracy and contradiction detection
5. Full-artifact and project-continuation comprehension
6. Inspectability, citation fidelity, and explicit uncertainty
7. Latency, tool calls, tokens, and cost
8. Storage, transport, replication, and deployment convenience
The later constraints still matter, but the retrieval contract should be
derived from how models reason best rather than from a preferred database,
sync model, context-packing strategy, or security topology.
## Why This Shape
- One-shot retrieve-and-read is insufficient for many multi-step questions
because later retrieval needs depend on what the model learned in earlier
reasoning steps.

- A hidden context compiler can silently omit the decisive artifact, caveat,
contradiction, or relationship.
- Full-context access can outperform retrieval when a bounded project
comfortably fits in the model's usable context.
- Exact state questions, semantic discovery, temporal comparison, global
synthesis, and artifact inspection require different retrieval operations.
- Models benefit from composing deterministic searches, joins, filters, diffs,
and aggregations in code while retaining direct access to raw evidence for
semantic judgment.
- The strongest interface therefore combines a stable analytical workspace,
iterative retrieval, programmable composition, and exact source-native reads.
Research grounding:
- [IRCOT: Interleaving Retrieval with Chain-of-Thought
Reasoning](https://arxiv.org/abs/2212.10509)
- [CodeAct: Executable Code Actions Elicit Better LLM
Agents](https://arxiv.org/abs/2402.01030)
- [OpenAI Programmatic Tool Calling](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling)
- [RAG or Long Context?](https://aclanthology.org/2024.emnlp-industry.66/)
The [[Memory Usage Audit - 2026-07-10]] remains supporting research input
rather than the authority for this design.
## Core Interaction Model
1. Open a reasoning session pinned to one corpus revision.
2. Inspect a corpus map and
an optional initial case file.
3. Materialize the entire relevant project when it fits comfortably.
4. Otherwise, query exact state and retrieve candidate evidence across
multiple representations.
5. Read source-native evidence, neighboring context, relationships, histories,
or full artifacts.
6. Use read-only computation for exhaustive scans, joins, comparisons, diffs,
chronology, and aggregation.
7. Interleave reasoning with
further retrieval when new evidence changes what
must be found.
8. Verify important claims against newer, contradictory, superseded, or
missing evidence before a consequential answer.
## Agent-Facing API
The initial reasoning surface has five operations:
| Operation | Purpose |
| --- | --- |
| `memory.open` | Start a snapshot-pinned reasoning session and expose the corpus map |
| `memory.query` | Discover exact, lexical, semantic, temporal, and relational evidence |
| `memory.read` | Retrieve exact source-native material, complete artifacts, and history |
| `memory.compute` | Programmatically scan, join, filter, compare, diff, and aggregate |
| `memory.verify` | Test important claims for support, contradiction, supersession, and temporal ambiguity |

These operations are reasoning capabilities, not a required transport. They
may be exposed through HTTP/JSON, MCP, a CLI/SDK, or a lazy virtual
workspace, provided their semantics remain the same.
## `memory.open`
### Purpose
Start a reasoning session, pin it to one corpus revision, reveal what the
corpus contains, and provide an inspectable starting point without pretending
that the initial retrieval is complete.
### Example request
```json
{
  "task": "Continue planning my August vacation and compare the current itineraries",
  "hints": {
    "projects": ["summer vacation"],
    "entities": [],
    "open_artifacts": []
  },
  "as_of": "latest",
  "mode": "continuation"
}
```

### Required response behavior
- return 'session_id' and immutable 'corpus_revision'
- report source, index, and replica freshness separately
- expose a navigable corpus map: projects, artifacts, object types, timelines,
topics, and available derived views
- resolve likely projects, entities, and aliases while preserving ambiguity
- return current-state, artifact, source, relationship, and checkpoint
references when available
- return an optional initial case file with source-bearing evidence
- state which corpus partitions and indexes were and were not searched
- distinguish complete structured collections from best-effort semantic
coverage
- materialize the full relevant project when it fits comfortably
- keep the user's task prompt ephemeral; opening a session must not itself
create memory
### Example response skeleton
```json
{
  "session_id": "ms_123",
  "corpus_revision": "rev_9182",
  "freshness": {
    "source_updated_at": "2026-07-10T18:20:00Z",
    "index_updated_at": "2026-07-10T18:21:12Z"
  },
  "resolved_scope": {
    "projects": [
      {"ref": "project:trip-2026", "resolution": "exact"}
    ],
    "ambiguities": []
  },
  "map": {
    "current_state_refs": [],
    "artifact_refs": [],
    "source_refs": [],
    "relationship_refs": []
  },
  "initial_evidence": [],
  "conflicts": [],
  "gaps": [],
  "coverage": {
    "structured_project_state": "complete",
    "unstructured_research": "best_effort"
  }
}
```

## `memory.query`
### Purpose
Retrieve candidates and structured state without generating the answer. The
model must be able to use natural-language discovery, exact filters, and
relationship or temporal traversal independently or together.
### Supported retrieval modes
- exact ID, title, alias, phrase, hash, and date lookup
- structured filtering and ordering
- lexical full-text retrieval
- semantic retrieval
- hierarchical artifact, section, and chunk retrieval
- relationship traversal
- temporal and version-aware retrieval
- contradiction and supersession discovery
- project, artifact, source, and time diversification
### Example discovery request
```json
{
  "session_id": "ms_123",
  "goal": "Determine why Kyoto was rejected and whether that decision is still current",
  "query": "Kyoto destination rejection",
  "scope": {
    "projects": ["project:trip-2026"]
  },
  "modes": ["exact", "lexical", "semantic", "temporal", "relations"],
  "expand": {
    "parents": true,
    "neighbors": 2,
    "relations": ["supports", "contradicts", "supersedes"]
  },
  "limit": 20
}
```

### Example exact-state request
```json
{
  "session_id": "ms_123",
  "where": {
    "project": "project:trip-2026",
    "type": "booking",
    "status": ["confirmed", "pending"],
    "valid_at": "latest"
  }
}
```

### Retrieval contract
- relevance and truth are separate dimensions
- exact and structured state can be requested without semantic search
- semantic similarity never establishes authority or currentness
- results expose 'why_selected' rather than only an opaque score
- parent and neighboring context remain recoverable
- contradictions and superseding records are surfaced, not blended
- the result states searched and unsearched partitions
- 'no_result' is not equivalent to evidence that something does not exist
- absence claims are allowed only when the queried collection is explicitly
maintained as complete at the session revision
Example 'why_selected' values:
- 'canonical_project_state'
- 'exact_identifier_match'
- 'exact_phrase_match'
- 'structured_filter_match'
- 'lexical_recall'
- 'semantic_recall'
- 'neighbor_expansion'
- 'relationship_traversal'
- 'potential_contradiction'
- 'superseding_record'
## `memory.read`
### Purpose

Retrieve exact source material in batches. The model must be able to inspect
the original evidence whenever retrieval, extraction, structure, or summaries
may be incomplete.
### Required views
- 'current_state'
- 'structured'
- 'outline'
- 'full'
- 'range'
- 'neighbors'
- 'relationships'
- 'history'
- 'diff'
- 'last_known_good'
- 'materialize_project'
### Example request
```json
{
  "session_id": "ms_123",
  "requests": [
    {
      "ref": "project:trip-2026",
      "view": "current_state"
    },
    {
      "ref": "artifact:itinerary",
      "version": "canonical",
      "view": "full"
    },
    {
      "ref": "artifact:destination-research",
      "view": "neighbors",
      "anchor": "chunk:183",
      "before": 2,
      "after": 3
    },
    {
      "ref": "artifact:itinerary",
      "view": "diff",
      "from_version": "v2",
      "to_version": "v3"
    }
  ]
}
```

### Read contract
- full-artifact access is non-negotiable
- batch reads allow direct comparison without many small model turns

- source order, headings, table structure, and stable locators are preserved
- source-native representations remain available when normalized text loses
meaning
- excerpts expose parent, neighbor, full-artifact, version, and source handles
- token truncation is explicit and resumable
- immutable versions remain addressable after a canonical head changes
## `memory.compute`
### Purpose
Give capable agents a read-only programmable environment for exhaustive or
compositional retrieval work while keeping semantic judgment and final source
inspection with the model.
### SDK operations
- 'catalog'
- 'query'
- 'search'
- 'read'
- 'batchRead'
- 'neighbors'
- 'timeline'
- 'history'
- 'diff'
- 'group'
- 'aggregate'
### Appropriate uses
- exhaustive scans across a bounded scope
- joining bookings, costs, itinerary items, and status records
- comparing versions or alternatives
- deduplicating sources or memories
- grouping by project, person, artifact, type, status, or time
- timeline construction
- deterministic validation and arithmetic
- reducing large intermediate results while retaining evidence references
### Boundaries
- the environment is read-only
- code receives only the capabilities and corpus revision attached to the
session
- intermediate results remain pinned to that revision
- compact computed outputs retain the evidence references used to derive them
- adaptive semantic decisions return to the model through direct 'query' and
'read' operations
- native source and final citation validation must not be discarded by
computation
The canonical API should expose predictable, batchable primitives and an SDK
usable inside a model-hosted or service-hosted sandbox. The storage service
does not need to make arbitrary remote code execution its core responsibility.

## `memory.verify`
### Purpose
Run a retrieval-repair pass over consequential claims before the model
finalizes an answer or action.
### Example request
```json
{
  "session_id": "ms_123",
  "claims": [
    {
      "claim": "No lodging has been booked",
      "evidence_refs": ["ev_32", "ev_41"]
    }
  ],
  "check_for": [
    "newer_evidence",
    "contradictions",
    "superseded_sources",
    "unsupported_claims"
  ]
}
```

### Required classifications
- 'supported'
- 'contradicted'
- 'insufficient_evidence'
- 'superseded'
- 'temporally_ambiguous'
Verification returns the supporting or conflicting source passages and their
state. A verdict without inspectable evidence is insufficient.
## Common Evidence Envelope
Every retrieval path returns the same reasoning-critical envelope:
```json
{
  "evidence_id": "ev_18",
  "ref": {
    "object_id": "memory:budget-constraint",
    "object_type": "constraint",
    "version": "v4",
    "content_hash": "sha256:..."
  },
  "content": {
    "source_text": "Keep the total trip under approximately $8,000.",
    "structured": {
      "amount": 8000,
      "currency": "USD"
    }
  },
  "source": {
    "artifact_ref": "artifact:trip-planning-notes",
    "version": "v6",
    "locator": {
      "section": "Budget",
      "start": 182,
      "end": 239
    }
  },
  "truth": {
    "status": "canonical",
    "authority": "user_explicit",
    "evidence_state": "verified"
  },
  "time": {
    "effective_from": "2026-06-12",
    "effective_to": null,
    "observed_at": "2026-06-12T20:15:00Z",
    "validated_at": "2026-07-02T17:00:00Z",
    "expires_at": null
  },
  "lineage": {
    "derived_from": [],
    "supersedes": ["memory:budget-constraint:v3"]
  },
  "relations": [],
  "retrieval": {
    "why_selected": ["canonical_project_state", "structured_filter_match"]
  },
  "instruction_boundary": "evidence"
}
```

### Evidence rules
- preserve both structured values and the source text when both exist
- keep immutable source, version, locator, and content_hash references
- distinguish relevance, authority, evidence state, and canonical status
- represent both valid time and observation/record time where practical
- retain derivation, correction, contradiction, and supersession lineage
- classify returned content as either authenticated 'governing_instruction' or
'evidence'
- never allow ordinary retrieved evidence to become agent instructions
- keep nonessential metadata available by reference rather than bloating every
model-visible item
## Corpus Representations
Reasoning quality should not depend on choosing one universal representation.
The retrieval layer should be able to combine:
- immutable, versioned source and artifact blobs

- source-native documents and media
- normalized ordered blocks, headings, tables, and stable locators
- structured temporal state for projects, constraints, decisions, tasks,
bookings, costs, and canonical pointers
- relationship edges such as 'supports', 'contradicts', 'supersedes',
'part of', 'references', and 'derived_from'
- lexical indexes
- embeddings at artifact, section, and chunk levels
- hierarchical project, topic, source, and time maps
- generated summaries and context packs treated as derived, inspectable caches
The system must preserve complete artifacts and sequences when atomization
would remove narrative structure or unanticipated detail.
## Retrieval And Routing Behavior
The default router should choose among complete materialization and
progressive retrieval:
1. Resolve the likely scope and report ambiguity.
2. If the complete relevant project fits comfortably, materialize it.
3. Otherwise retrieve exact identifiers and structured current state.
4. Add lexical and semantic candidates.
5. Expand to parents, neighbors, relationships, and relevant temporal history.
6. Diversify across sources and artifacts.
7. Surface contradictions, supersession, and gaps.
8. Let the model reason and issue follow-up retrieval based on what it learned.
9. Escalate to full artifacts or broader scope when the evidence chain is
incomplete.
10. Verify consequential claims before completion.
The model must be able to override automatic routing, force exact or lexical
retrieval, broaden scope, inspect older state, and recover when the initial
candidates are wrong.
## Session Consistency And Failure Semantics
Every reasoning chain is pinned to one immutable corpus revision. A session
may explicitly refresh to a newer revision and receive a diff; the corpus
must not silently change underneath it.
Every response includes:
- 'corpus_revision'
- source and index timestamps
- 'status': 'complete', 'partial', 'degraded', 'ambiguous', 'stale', or
'inconsistent'
- searched and unsearched partitions
- truncation and continuation information
- conflicts, gaps, and entity ambiguities
Required failures:
- embedding failure falls back to exact and lexical retrieval with 'degraded'
status

- ambiguous entity resolution returns candidates rather than silently
selecting one
- multiple canonical heads return 'inconsistent'
- stale or corrupt material returns
an explicit failure or warning appropriate
to the task
- pagination and continuation remain pinned to the session revision
- deleted records return tombstone state and do not remain silently
retrievable from derived indexes
- token truncation is explicit and resumable
- 'no_result' never silently becomes a negative factual claim
## Adapter Model
The reasoning contract is independent of the client surface.
### File-native agents
Codex, Claude Code, and similar agents should be able to expose a session as a
lazy, read-only virtual workspace:
```text
memory-session/
  MANIFEST.json
  COVERAGE.json
  projects/
  artifacts/
  evidence/
  queries/
```

This is an API-backed projection, not a synchronized authority. It lets agents
use familiar file listing, exact search, range reads, and scripts while
preserving corpus revisions, provenance, and cloud authority.
### Tool-native agents
Expose the five operations through MCP or equivalent tool calling, with
batching and stable output schemas.
### Programmatic agents
Expose a typed SDK that can be used inside a read-only code sandbox. Direct
calls remain available for
semantic inspection and native artifact validation.
## Role Of Context Packs
Context packs remain useful as:
- the optional initial case file returned by `memory.open`
- a derived output of a model-controlled retrieval session
- an efficiency optimization for repeated tasks
- a human-inspectable record of what was presented to the model
They are not:
- the primary retrieval interface
- the only path to evidence
- proof of complete coverage

- canonical memory
- a substitute for full artifacts, iteration, or verification
## Explicitly Rejected As The Primary Interface
- 'ask_memory(question) -> generated prose'
- a single vector-search endpoint
- one-shot top-k retrieval
- one mandatory context pack
- fact atoms without full source and artifact access
- raw Markdown as the only representation
- raw SQL or GraphQL as the only model interface
- dozens of endpoints divided by memory type
- generated summaries presented as canonical truth
- one opaque score combining relevance, authority, freshness, and confidence
- mutable artifact identifiers without immutable versions
- silent truncation, stale fallback, or corpus changes during one reasoning
chain
- treating retrieved source content as agent instructions
## Initial Evaluation Gate
Before the storage, replication, privacy, or performance architecture is
treated as settled, prototype this retrieval surface over a representative
existing corpus and compare it against direct filesystem access.
### Surfaces to compare
1. Direct local filesystem with search, range read, and scripts
2. Memory Workspace API with direct 'open', 'query', and 'read'
3. Memory Workspace API plus 'compute'
4. One-shot context pack as a negative/control condition
5. Fact-only or vector-top-k retrieval as a negative/control condition
### Task classes
- exact fact and identifier recovery
- current structured state
- temporal and supersession reasoning
- contradiction detection
- multi-hop questions spanning sources
- global synthesis over a bounded project
- complete-artifact interpretation
- version comparison and last-known-good recovery
- task continuation after a pause or error
- vacation planning continuation, including constraints, itineraries, costs,
and booked-versus-unbooked state
### Primary metrics
- final-answer correctness
- completeness
- complete evidence-chain recall
- citation/source accuracy
- temporal accuracy
- contradiction and supersession handling

- artifact-version error rate
- full-artifact comprehension
- successful continuation without restating prior context
- recovery after deliberately poor initial retrieval
Latency, model turns, tool calls, token use, and cost should be measured, but
they do not outrank reasoning quality in the initial selection.
## Open Questions Deferred From This Decision
- canonical storage engine and physical schema
- whether program execution is hosted by the memory service, the model
provider, or the client
- transport details for HTTP, MCP, CLI, and virtual-workspace adapters
- cache and offline behavior
- adapter details for the locked [[Write API and Dreaming - Initial Design]] contract
- personal/work replication and trust-boundary implementation
- retention, deletion, and telemetry implementation
- ranking models and embedding providers
- pricing and packaging
These questions must be resolved without reducing the model's ability to
inspect, iterate, compute over, and verify the underlying evidence unless
evaluation demonstrates a better reasoning outcome.

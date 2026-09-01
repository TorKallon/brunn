Created: 2026-07-10 21:30 PDT
Updated: 2026-07-10 23:30 PDT
Status: Active

Related: [[Projects/Metis/Metis|Metis]], [[Projects/Aether/Aether|Aether]], [[Active projects]]

## Purpose
Brunn is a user-owned, portable context and durable-work layer for agents. It is intended to let Codex, Claude Code, personal laptop agents, OpenClaw/Hermes-style systems, and future agent runtimes learn, continue, and iteratively advance complex work without turning every transcript into permanent memory or collapsing every trust domain into one database.

## Working name
**Brunn** is the project name.

The name references Villa Brunn in William Gibson's *Neuromancer*: a persistent place containing history, continuity, secrets, and control structures, rather than an AI itself. The association with optical stray light and unintended context leakage is a known negative connotation. If the project becomes public, pair the name with a descriptive product category rather than relying on the bare name alone. The descriptor is deliberately deferred.

## Product thesis
A portable agent context layer should preserve both what an agent has learned and the live state of work: evidence, decisions, hypotheses, plans, artifacts, computations, unresolved questions, rejected paths, checkpoints, and the next safe action. A later agent should be able to resume the work, inspect how the current state was reached, revise it as new evidence arrives, and leave a better durable state behind.

This is not primarily a notes app, wiki, synchronized folder, transcript archive, retrieval product, or memory database. Human-readable notes and Obsidian are useful projections and prototype inputs, not the product boundary or canonical interaction model. The product is agent-first: its main job is durable machine continuity across research, planning, implementation, operations, verification, and handoff.

## Strongest wedge
- one user-owned context substrate across otherwise disconnected agent ecosystems
- either one unified personal fabric or multiple physically separate vault instances under the same protocol
- selective implicit capture rather than transcript hoarding
- governed directional context movement rather than one undifferentiated global brain
- provenance, corrections, freshness, contradiction handling, and deletion
- open adapters and exports that reduce agent-vendor lock-in
- a durable workspace that can be reopened, queried, computed over, revised, verified, checkpointed, and handed to the next agent
- full source and artifact access as part of doing work, rather than retrieval as the product

## Governing architecture
1. **Evidence layer:** immutable source episodes, artifacts, corrections, versions, provenance, authority, valid time, and transaction time.
2. **Online capture:** a small `memory.save` surface for ordinary persistence and `memory.stage` for material that must be inspected before persistence.
3. **Typed knowledge and learning:** preferences, instructions, decisions, commitments, reusable patterns, research findings, relationships, corrections, and learned operational rules.
4. **Dreaming:** slow, bounded, source-preserving consolidation that builds shadow revisions and treats derived memory as a reversible optimization over the evidence ledger.
5. **Durable reasoning workspace:** snapshot-pinned working sets containing goals, current state, plans, hypotheses, artifacts, computations, open questions, rejected approaches, checkpoints, and resumable next actions. Core operations include `memory.open`, `memory.query`, `memory.read`, `memory.compute`, `memory.verify`, and versioned workspace updates.
6. **Cloud authority and adapters:** one authoritative personal service, disposable client caches, open protocol surfaces, and integrations for agent runtimes.
7. **Replication and trust:** signed, encrypted, directional snapshots or deltas between physically separate instances when a namespace is not a sufficient boundary.
8. **Human control:** remember, correct, forget, do not save, inspect provenance, audit use, export, and review consequential proposals.

The governing dreaming rule is: **the evidence ledger is memory; the consolidated layer is a disposable, versioned optimization over that evidence.**

## Rourke trust model
Personal memory and OpenAI work memory must remain physically separate.

The allowed direction is:

```text
personal cloud authority
  -> signed and encrypted snapshot/delta outbox
  -> one-way work importer
  -> complete or policy-scoped work-local personal replica
  -> Work Codex
```

Personal context may flow into the work environment. Work prompts, queries, embeddings, outputs, telemetry, derived state, and acknowledgements must never flow back to the personal service. A work-generated recommendation returns to personal memory only through an explicit human-reviewed declassification step.

## Initial product shape
- one cloud-authoritative personal service with optional disposable encrypted caches
- five initial semantic memory classes: preference/instruction, decision, project state, reusable pattern, and research finding
- first-class source, artifact, asset, relationship, correction, and checkpoint objects
- Codex local/cloud as the first integration, including the one-way personal-to-work path
- explicit remember/correct/forget plus selective end-of-turn capture
- task-scoped resumable checkpoints independent of durable semantic promotion
- Phase 0 shadow-only dreaming before any automatic active-memory mutation
- review/audit views plus Markdown and JSON export
- complete deletion propagation through canonical state, indexes, embeddings, caches, derived views, exports, and replicas

## Evaluation posture
The evaluation target is successful continuation and advancement of real work, not retrieval recall in isolation. Reasoning quality, source fidelity, durable state quality, and ability to iterate outrank storage or deployment convenience. Evaluations should compare:
1. direct filesystem access with search, range reads, and scripts
2. an agent-first workspace with model-directed `open`, `query`, `read`, `compute`, `verify`, checkpoint, and revision behavior
3. a fixed handoff/context pack as a control condition

Primary tests should cover project resumption, iterative planning after constraints change, quantitative artifact work, temporal and supersession reasoning, contradictions, operational learning, incident continuation, full-source inspection, verification before action, durable handoff construction, and booked-versus-proposed vacation state. Representative workloads include Warmind/Charlemagne performance and incident work, StarRupture factory planning, Switzerland trip planning, and Brunn's own evolving design.

## Relationship to the vault
- **Metis** organizes the human knowledge base and provides the first representative corpus.
- **Brunn** is the portable agent context and durable-work product being designed over that kind of corpus.
- **Aether** is a likely personal-agent client and integration environment.

The Obsidian vault is therefore an excellent prototype and evaluation corpus, but raw Markdown is not assumed to be Brunn's only or canonical storage representation.

## Current status
The concept and three initial technical contracts have been separated and captured in the vault. The first retrieval-readiness benchmark is complete, but it tested too narrow a slice of the product. The evaluation is being expanded around real agent work: resuming and advancing Warmind/Charlemagne investigations, iterating StarRupture factory designs, planning the Switzerland trip under changing constraints, and continuing Brunn itself. Storage, schema, deployment, and commercial choices remain open.

## Active docs
- [[Records/Retained PDFs/Brunn/Portable Personal Context Layer - 2026-07-10.pdf|Source PDF]]
- [[Portable Personal Context Layer]]
- [[Write API and Dreaming - Initial Design]]
- [[Dreaming Architecture and Plan - Initial Design]]
- [[Retrieval API - Initial Design]]
- [[Projects/Brunn/Retrieval evaluation plan|Retrieval evaluation plan]]
- [[Projects/Brunn/Retrieval evaluation results - 2026-07-10|Retrieval evaluation results - 2026-07-10]]
- [[Projects/Brunn/Decisions|Decisions]]
- [[Projects/Brunn/Open questions|Open questions]]
- [[Projects/Brunn/Worklog|Worklog]]

## Next actions
- run the expanded agent-work evaluation with model-directed workspace calls, direct filesystem tools, and fixed handoff packs
- evaluate continuation, iteration, computation, verification, and durable next-state quality rather than retrieval recall alone
- define the first typed object model and common evidence envelope against real vault material
- choose the narrowest Codex integration path for `memory.open`, explicit `memory.save`, and task checkpoints
- keep dreaming shadow-only until retrieval and transition evaluations demonstrate no material regression

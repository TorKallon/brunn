Created: 2026-07-10 21:30 PDT
Updated: 2026-07-10 21:30 PDT
Status: Active

Related: [[Projects/Metis/Metis|Metis]], [[Projects/Aether/Aether|Aether]], [[Active projects]]

## Purpose
Brunn is a user-owned, portable context and memory layer for agents. It is intended to let Codex, Claude Code, personal laptop agents, OpenClaw/Hermes-style systems, and future agent runtimes continue work from durable context without turning every transcript into permanent memory or collapsing every trust domain into one database.

## Working name
**Brunn** is the project name.

The name references Villa Brunn in William Gibson's *Neuromancer*: a persistent place containing history, continuity, secrets, and control structures, rather than an AI itself. The association with optical stray light and unintended context leakage is a known negative connotation. If the project becomes public, pair the name with a descriptive product category rather than relying on the bare name alone. The descriptor is deliberately deferred.

## Product thesis
A portable memory control plane should give a person's agents durable continuity while preserving user ownership, inspectability, provenance, freshness, reversibility, and user-chosen trust boundaries.

This is not primarily a notes app, wiki, synchronized folder, transcript archive, or vector database. Human-readable notes are an important projection and export format, but the product's main job is machine continuity: preserving the evidence, state, decisions, artifacts, and learned patterns needed for a later agent to continue intelligently.

## Strongest wedge
- one user-owned context substrate across otherwise disconnected agent ecosystems
- either one unified personal fabric or multiple physically separate vault instances under the same protocol
- selective implicit capture rather than transcript hoarding
- governed directional context movement rather than one undifferentiated global brain
- provenance, corrections, freshness, contradiction handling, and deletion
- open adapters and exports that reduce agent-vendor lock-in
- reasoning-first retrieval with full source and artifact access, not opaque one-shot top-k recall

## Governing architecture
1. **Evidence layer:** immutable source episodes, artifacts, corrections, versions, provenance, authority, valid time, and transaction time.
2. **Online capture:** a small `memory.save` surface for ordinary persistence and `memory.stage` for material that must be inspected before persistence.
3. **Typed memory and project state:** preferences, instructions, decisions, commitments, project state, reusable patterns, research findings, relationships, corrections, and versioned artifacts.
4. **Dreaming:** slow, bounded, source-preserving consolidation that builds shadow revisions and treats derived memory as a reversible optimization over the evidence ledger.
5. **Reasoning workspace:** snapshot-pinned `memory.open`, `memory.query`, `memory.read`, `memory.compute`, and `memory.verify` operations with full-artifact escalation.
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
Reasoning quality outranks storage or deployment convenience in the initial design. The first evaluation should compare:
1. direct filesystem access with search, range reads, and scripts
2. Memory Workspace `open`, `query`, and `read`
3. Memory Workspace plus `compute`
4. a one-shot context pack as a negative/control condition
5. fact-only or vector-top-k retrieval as a negative/control condition

Primary tests should cover exact identifiers, current structured state, temporal and supersession reasoning, contradictions, multi-hop evidence, complete artifacts, project continuation, recovery from poor initial retrieval, and booked-versus-unbooked vacation state.

## Relationship to the vault
- **Metis** organizes the human knowledge base and provides the first representative corpus.
- **Brunn** is the portable agent-memory product and protocol being designed over that kind of corpus.
- **Aether** is a likely personal-agent client and integration environment.

The Obsidian vault is therefore an excellent prototype and evaluation corpus, but raw Markdown is not assumed to be Brunn's only or canonical storage representation.

## Current status
The concept and three initial technical contracts have been separated and captured in the vault. The write/capture, dreaming, and retrieval designs are marked `locked-initial-design`; storage, schema, deployment, and commercial choices remain open.

## Active docs
- [[Records/Retained PDFs/Brunn/Portable Personal Context Layer - 2026-07-10.pdf|Source PDF]]
- [[Portable Personal Context Layer]]
- [[Write API and Dreaming - Initial Design]]
- [[Dreaming Architecture and Plan - Initial Design]]
- [[Retrieval API - Initial Design]]
- [[Projects/Brunn/Decisions|Decisions]]
- [[Projects/Brunn/Open questions|Open questions]]
- [[Projects/Brunn/Worklog|Worklog]]

## Next actions
- define the smallest representative vault corpus and frozen continuation/evidence test suite
- prototype the reasoning-first retrieval surface before choosing the final storage engine
- define the first typed object model and common evidence envelope against real vault material
- choose the narrowest Codex integration path for `memory.open`, explicit `memory.save`, and task checkpoints
- keep dreaming shadow-only until retrieval and transition evaluations demonstrate no material regression

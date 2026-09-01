Created: 2026-07-10 21:30 PDT
Updated: 2026-07-10 21:30 PDT

Related: [[Brunn]], [[Portable Personal Context Layer]], [[Write API and Dreaming - Initial Design]], [[Dreaming Architecture and Plan - Initial Design]], [[Retrieval API - Initial Design]]

## Project decisions
- The working project name is **Brunn**.
- The name is a *Neuromancer* reference to Villa Brunn, a persistent place and substrate rather than a specific AI.
- The negative optical meaning of stray light is acknowledged. A descriptive public-facing product category may be added later; no descriptor is selected yet.
- Brunn is a distinct project from Metis. Metis is the knowledge-base system; Brunn is the portable agent-memory product and protocol.

## Product decisions
- The product is an agent memory and context layer, not primarily a notes app, wiki, transcript archive, synchronized folder, or vector database.
- Memory must be user-owned, portable, inspectable, reversible, provenance-bearing, and freshness-aware.
- The protocol must support either one logical personal fabric or physically separate trust-domain instances.
- Selective implicit capture is allowed only under clear policy. Explicit remember, correct, forget, do-not-save, and show-what-was-used controls remain first class.
- Full source artifacts and evidence remain available; summaries, embeddings, indexes, and context packs are derived aids rather than truth boundaries.

## Authority and trust decisions
- Rourke's personal instance is cloud-authoritative. Local personal caches are disposable and never become peers or authorities.
- Rourke's personal and OpenAI work memories are physically separate.
- The allowed automated flow is personal to work through signed, encrypted, application-layer one-way replication.
- No work query, prompt, embedding, output, telemetry, acknowledgement, or derived state may reach the personal service.
- Imported personal data remains visibly tagged, non-exportable, and non-promotable into ordinary work memory by default.
- Work-generated output can return to personal memory only through an explicit, human-reviewed declassification step.

## Write and capture decisions
- Fast online capture and slow offline consolidation are separate planes.
- Ordinary writes use one evidence-backed `memory.save` operation.
- `memory.stage` is reserved for files, directories, archives, attachments, and content packs that require inspection before persistence.
- Every turn may receive a lightweight formation evaluation, but most turns should produce `no_op` for durable semantic memory.
- Task checkpoints are refreshed independently from semantic-memory promotion.
- Explicit corrections take effect immediately and preserve immutable history.
- Online semantic similarity may suggest a candidate but may never destructively merge records.

## Dreaming decisions
- The evidence ledger is immutable and authoritative; derived memory is disposable, versioned, and reversible.
- Dreaming is bounded by region, snapshot-pinned, event-aware, source-preserving, and initially shadow-only.
- Deterministic maintenance and model-based consolidation are separate loops.
- Every candidate revision requires preservation, authority, temporal, policy, and downstream retrieval checks.
- Safe derived aids may eventually promote automatically after gates. Canonical facts, user preferences, merges, supersession, sensitive judgments, scope changes, and deletion remain review-only or explicitly authorized.
- A dream may never delete source evidence as a side effect.

## Retrieval decisions
- The initial interface is a stateful, read-only Memory Workspace API optimized for reasoning quality.
- Sessions are pinned to an immutable corpus revision.
- The core operations are `memory.open`, `memory.query`, `memory.read`, `memory.compute`, and `memory.verify`.
- Complete project materialization is preferred when it comfortably fits; progressive exact, structured, lexical, semantic, temporal, relational, and hierarchical retrieval handles larger scopes.
- Relevance, authority, canonicality, freshness, and evidence state remain separate dimensions.
- Full-artifact access, neighboring context, version history, contradiction discovery, explicit coverage, and resumable truncation are required.
- Context packs remain optional starting hints or derived views, not the primary interface or proof of complete coverage.

## Evaluation decisions
- Retrieval must be prototyped against a representative existing corpus before storage, replication, privacy, or performance architecture is treated as settled.
- Final-answer correctness, completeness, evidence-chain recall, temporal accuracy, contradiction handling, source fidelity, and project continuation outrank latency and cost in the first selection.
- Dreaming must be evaluated separately at capture, transition, retrieval, and downstream-reasoning layers.


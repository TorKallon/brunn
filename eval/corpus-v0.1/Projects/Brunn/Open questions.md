Created: 2026-07-10 21:30 PDT
Updated: 2026-07-10 21:30 PDT

Related: [[Brunn]], [[Projects/Brunn/Decisions|Decisions]]

## Immediate prototype questions
- What is the smallest representative slice of the vault for the first retrieval experiment?
- Which frozen tasks become the initial gold set: vacation planning, project continuation, exact identifiers, temporal state, contradiction repair, artifact comparison, or a mix?
- What result would justify the Memory Workspace abstraction over direct filesystem tools?
- Which existing Codex surface is the narrowest viable first client: plugin, MCP tools, CLI, local API, or a file-native projection?
- How much of `memory.open`, `memory.query`, `memory.read`, `memory.compute`, and `memory.verify` is needed for the first meaningful comparison?

## Object and storage model
- What is the canonical storage engine and physical schema?
- Which objects are primitive: evidence events, memories, project state, artifacts, assets, relationships, checkpoints, corrections, tombstones, and derived views?
- How are valid time, transaction time, authority, epistemic state, sensitivity, scope, and lineage represented consistently?
- Which parts of a complete artifact are normalized into blocks, and which remain source-native only?
- Where does read-only program execution run: memory service, model provider, or client?
- What are the cache, offline draft, and stale-revision behaviors?
- Which lexical, embedding, and ranking systems should be used initially?

## Capture and staging
- What exact policy decides `memory.save` versus `no_op` at turn end?
- How are sensitive or inferred candidates presented for review without interrupting ordinary work?
- What are the retention periods for task checkpoints, provisional memories, abandoned stages, and selected turn evidence?
- Which integrations can provide reliable end-of-turn hooks?
- Where are staged files stored, and what are the limits, pricing, quarantine, and lifecycle rules for large opaque assets?
- How should completion receipts expose index lag and partial derivative readiness?

## Dreaming
- How should dirty score, recurrence, evidence volume, cooldown, region size, and compute budget be tuned from observed corpus behavior?
- Which dream proposals can become automatically promotable after Phase 0 evidence?
- What review experience is needed for canonical merges, deletion, sensitive inference, and cross-scope movement?
- When, if ever, should task-specific consolidation programs supplement the initial strong-dreamer, deterministic-worker, independent-verifier topology?
- Which cross-model tests are required to avoid overfitting memory to the model that authored it?

## Trust, replication, and deletion
- What exact service boundary and key model implement personal-to-work signed snapshot/delta replication?
- How is a neutral outbox made unable to read work acknowledgements while still allowing operations to detect delivery failures?
- What full-replica, project-scoped, field-scoped, and ephemeral export policies should the product expose?
- How are imported personal objects prevented from entering work-memory promotion, training, export, telemetry, and retention paths?
- What deletion receipt proves propagation through canonical objects, chunks, indexes, embeddings, caches, derived views, exports, and replicas?
- What assurance language accurately distinguishes application-layer one-way replication from a physical data diode?

## Product and commercial shape
- Who is the first user beyond Rourke, and which continuation problem is painful enough to define the initial wedge?
- What public descriptor, if any, should accompany Brunn?
- What human control surface is required for trust without turning the product into a manual filing system?
- Which adapters and export formats are required for credible portability?
- What are the pricing, packaging, retention, and large-asset limits?

## Referenced input not present in the PDF
- The source documents refer to `Memory Usage Audit - 2026-07-10`, but that note was not included in the supplied PDF. Locate or recreate it before treating the research record as complete.


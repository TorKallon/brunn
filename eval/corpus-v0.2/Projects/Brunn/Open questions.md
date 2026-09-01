Created: 2026-07-10 21:30 PDT
Updated: 2026-07-10 22:56 PDT

Related: [[Brunn]], [[Projects/Brunn/Decisions|Decisions]]

## First prototype answers
- The first corpus contains 44 non-private project notes from Metis, N24 RaceWatch, Home Network Improvements, and Brunn.
- The frozen set contains 20 questions and 53 gold evidence items spanning exact facts, policy, current state, contradictions, quantitative evidence, continuation, authority, and multi-source reasoning.
- The deterministic workspace did not beat one-shot top-k on aggregate evidence recall or cost, so the abstraction has not yet cleared its justification gate.
- The first prototype exercised `memory.open`, `memory.query`, and `memory.read` behavior. Model-directed `memory.compute` and `memory.verify` remain untested.

## Immediate evaluation questions
- Does a model controlling follow-up workspace calls produce more complete, better-cited answers than one-shot context on blinded cases?
- What untouched holdout set is large and varied enough to detect policy overfitting?
- What improvement threshold justifies the added calls and context cost of a stateful workspace?
- Which of the three remaining workspace failures are fixed by model-directed exploration rather than more deterministic ranking rules?
- Which existing Codex surface is the narrowest viable first client: plugin, MCP tools, CLI, local API, or a file-native projection?

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

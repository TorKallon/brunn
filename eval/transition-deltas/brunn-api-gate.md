# Brunn transition decision: persistent API gate

Observed at: 2026-07-11T08:00:00Z
Authority: explicit project-owner direction informed by the completed evaluation

Proceed from evaluation into implementation. The first build must use a persistent, snapshot-pinned workspace rather than the process-per-operation BM25 shell prototype.

The first implementation gate requires:

- reopen a parent checkpoint created at corpus revision N
- refresh into revision N+1 and expose the changed evidence or constraint as a diff
- combine exact and lexical retrieval with a pluggable semantic retrieval path
- batch query and read operations through a typed API
- commit an immutable child checkpoint that names its parent and preserves source references
- avoid reconstructing the complete prior corpus when the checkpoint and delta are sufficient

The transition harness target is no more than four completed workspace calls per case while matching the direct-filesystem answer claims. Cumulative cached replay and uncached input must be reported separately. The BM25 shell prototype remains a regression fixture, not the target architecture.

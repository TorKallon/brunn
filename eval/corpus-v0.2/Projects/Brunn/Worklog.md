Created: 2026-07-10 21:30 PDT
Updated: 2026-07-10 22:56 PDT

Related: [[Brunn]]

## 2026-07-10
- Selected **Brunn** as the project name, referencing Villa Brunn from *Neuromancer*.
- Deferred any public-facing descriptor; recorded the optical leakage connotation as a naming consideration rather than a blocker.
- Read and visually verified the full 48-page Portable Personal Context Layer PDF.
- Retained the original source as [[Records/Retained PDFs/Brunn/Portable Personal Context Layer - 2026-07-10.pdf|Portable Personal Context Layer PDF]] and verified its SHA-256 checksum after copying.
- Identified four independent source documents and converted them into separate Markdown notes:
  - [[Portable Personal Context Layer]]
  - [[Write API and Dreaming - Initial Design]]
  - [[Dreaming Architecture and Plan - Initial Design]]
  - [[Retrieval API - Initial Design]]
- Reconstructed malformed PDF text-layer content against rendered pages, including tables, JSON examples, the Mermaid architecture graph, state diagrams, and file-workspace examples.
- Added the Brunn overview, decision register, open-question register, and active-project routing.
- Established the first proposed execution step: evaluate a reasoning-first retrieval prototype over a representative vault corpus before choosing the final storage engine.
- Built a reproducible standard-library Python benchmark at `/Users/Shared/projects/brunn` with 20 frozen questions and 53 gold evidence items across Metis, N24 RaceWatch, Home Network Improvements, and Brunn.
- Compared direct full-file access, one-shot BM25 top-k, and a minimal stateful Memory Workspace.
- Preserved the initial run, where the workspace passed 12 of 20 cases, then made one policy-level tuning pass without changing the manifest or corpus.
- Policy 0.2 improved the workspace to 17 of 20 cases and 46 of 53 evidence items while keeping P95 context at 11,999 characters.
- One-shot top-k also passed 17 of 20 cases, recovered 48 of 53 evidence items, and used about 1,070 mean estimated tokens versus 2,932 for the workspace.
- Concluded that the workspace hypothesis is not yet demonstrated. Froze policy 0.2 as a regression baseline and selected a blinded model-answer evaluation with an untouched holdout set as the next gate.
- Recorded the complete result in [[Projects/Brunn/Retrieval evaluation results - 2026-07-10|Retrieval evaluation results - 2026-07-10]].
- Retained the exact evaluated corpus at `/Users/Shared/projects/brunn/eval/corpus-v0.1`; its SHA-256 matches the recorded run, allowing the live vault to evolve without moving the benchmark.

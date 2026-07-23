# Rupture Ops benchmark corpus v0.1

Frozen: 2026-07-11

This corpus is a private evaluation fixture assembled from the local RuptureOps
vault section, the RuptureOps repository, imported player prompt history, and
selected Codex interaction evidence. It is intended to compare:

1. a bounded fixed context pack;
2. direct access to local Markdown, structured text, and source files; and
3. Straylight-compatible workspace architectures.

The corpus preserves its source directory shape where practical. Repository
material is nested under `Projects/RuptureOps/Repository` so one project scope
can expose product, import, code, and artifact state together.

Binary site images, the generated app icon, and warning audio are retained for
a future multimodal/object-store lane. The current agent-work loader indexes
their text sidecars and hashes but does not send binary pixels or audio to the
model. No current scored claim requires visual inspection of binary content.

This fixture contains personal play state and third-party source-derived
material. Do not publish it.

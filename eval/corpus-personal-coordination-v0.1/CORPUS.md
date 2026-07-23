# Personal coordination frozen corpus v0.1

This fixture is wholly synthetic and pseudonymized. Names, contacts, places,
times, and identifiers do not map to a real person or account. Contact values
use reserved example domains. The corpus contains no street addresses,
booking codes, vault paths, or reversible mappings.

## Generic kernel

Every stable semantic referent has one `object_id` and may carry multiple
compatible type profiles. Person, organization, group, place, event,
arrangement, resource, work-item, and artifact are profiles, not separate
stores. Facts are source-bearing claims. Roles and commitments are immutable
qualified-relation revisions. Event times use typed temporal and recurrence
specifications. Independent state assignments are claims. Checkpoints preserve
current state, decisions, open questions, next actions, and source artifacts.

Source snapshots remain intact when they conflict. A later authoritative
revision may supersede an earlier claim without deleting it. Names and aliases
are claims rather than identity keys; `possibly_same_as` is not `same_as`.
Person dossiers and briefs are rebuildable projections over claims, relations,
events, arrangements, work-items, and artifacts. Inferred assessments retain
their epistemic label and never silently become traits.

Sensitivity is attached to fields and relations. Projections enforce purpose,
audience, and fact-scoped authority while retaining provenance and redaction
receipts. A read-only credential can reason over a pinned snapshot but cannot
persist a checkpoint or mutate corpus or staged state. Vacation planning and
game-night continuity use this same kernel.

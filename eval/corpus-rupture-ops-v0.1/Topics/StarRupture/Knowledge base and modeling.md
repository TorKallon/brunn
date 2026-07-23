Created: 2026-07-10
Updated: 2026-07-11

Related: [[Topics/StarRupture/StarRupture|StarRupture]], [[Topics/StarRupture/Game mechanics - Update 1|Game mechanics - Update 1]], [[Topics/StarRupture/Early Access - Update 1|Early Access - Update 1]], [[Projects/RuptureOps/RuptureOps|RuptureOps]]

## Purpose

Human-facing boundary for the versioned StarRupture knowledge model that feeds RuptureOps. Implementation, schemas, refresh commands, and detailed validation live with the code in the repo.

## Ownership split

- Product planning and durable game interpretation stay in this vault.
- Pipeline code, curated machine-readable facts, snapshot manifests, normalized data, models, and generated local artifacts live in `/Users/Shared/projects/ruptureops`.
- Detailed operating guide: [repo data-corpus documentation](</Users/Shared/projects/ruptureops/docs/data-corpus.md>).
- Current snapshot pointer: [current.json](</Users/Shared/projects/ruptureops/data/current.json>).
- Local SQLite knowledge base: [star-rupture.sqlite3](</Users/Shared/projects/ruptureops/data/star-rupture.sqlite3>).
- Imported analysis ledger: [Star Rupture Game Analysis import](</Users/Shared/projects/ruptureops/imports/star-rupture-game-analysis__20260708T190137Z/README.md>).

## Version model

The corpus keeps two independent clocks:

```text
game build axis       Early Access -> Update 1 -> hotfix lineage
source snapshot axis  SRDB version + claimed data build + capture time
```

This prevents an April 9 Update 1 number from silently becoming a later-hotfix number, a source correction from overwriting history, or hidden/test content from leaking into player-facing plans.

## Reasoning rules

- Pin numerical answers and scenarios to a source snapshot.
- Pin build-specific mechanics to an immutable Steam build when evidence supports it.
- Preserve raw IDs, provenance, spelling, hidden state, and unresolved references.
- Keep aliases and provisional derived values separate from source facts.
- Treat official Creepy Jar announcements as primary for release intent and SRDB as the designated structured-data source.
- Keep intended design changes, fixes, inferred facts, migrations, and community-tool behavior distinct.

## Current model boundary

The production model handles recursive recipes, exact and rounded machine demand, raw extraction rates, one-time research costs, provisional power/capacity totals, saved scenarios, and explicit unresolved-production records.

The July 11 import adds a separate player-state model, six medium-confidence recipe/producer overrides for Ore Excavator v2 purity variants, and medium-confidence food-effect assumptions. These remain visibly curated; they do not overwrite raw source records.

It does not yet claim to solve floor-plan geometry, Drone Rail priority/throughput, storage buffers, Cargo transit, power islands, verified Base Core capacity semantics, Fire Wave downtime, global alternate-recipe optimization, unlock critical paths, loot expected value, or verified live combat balance.

## Source and reuse boundary

The source permits crawling but publishes no explicit data/content reuse license. The mirrored raw archive, normalized corpus, and derived SQLite database are private research assets. Keep the RuptureOps repository private unless permission or a separately licensed data strategy is established.

The imported package is also private: it contains personalized save state, player screenshots, conversation-derived decisions, and a duplicate third-party corpus. Only its manifests and promoted, provenance-labeled facts belong in version control.

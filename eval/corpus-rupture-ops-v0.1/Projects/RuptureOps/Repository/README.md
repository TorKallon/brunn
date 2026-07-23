# RuptureOps

RuptureOps is an unofficial iOS companion for StarRupture. The product is
intended to grow across factory planning, production-chain calculations,
progression, map knowledge, saved plans, and other in-game reference tools.

The repository currently contains the versioned StarRupture knowledge and data
pipeline that will underpin the app. The SwiftUI application has not been
scaffolded yet.

This repository and its normalized corpus must remain private unless separate
reuse permission is obtained from the source owner or the app adopts a
different licensed-data strategy. Do not push the current corpus to a public
remote.

## Repository map

- `scripts/` — source capture, normalization, SQLite build, query, and validation tools.
- `sources/` — curated aliases, game-build history, versioned mechanic facts,
  provenance-labeled producer overrides, and consumable-effect assumptions.
- `models/` — saved reproducible planning scenarios, current player-save state,
  and the deduplicated play-history model.
- `imports/` — tracked provenance for external research and user-authored prompt
  history; third-party bundles and extracted duplicate corpora remain local and
  ignored.
- `data/current.json` — pointer to the current immutable source snapshot.
- `data/snapshots/` — locally tracked manifests and normalized snapshot data;
  private-use only under the current source boundary.
- `data/star-rupture.sqlite3` — generated local knowledge database; ignored by Git.
- `data/snapshots/*/raw/` — private raw captures and mirrored assets; ignored by Git.

The verified July 11 Star Rupture Game Analysis import is documented at
`imports/star-rupture-game-analysis__20260708T190137Z/README.md`. Its older
SQLite/source exports were deduplicated rather than installed as a competing
snapshot.

The three user-authored prompt dumps are documented at
`imports/player-prompt-history__20260711/README.md`. Their exact texts are
tracked for private history, while overlapping facts are normalized once into
`models/rourke-update1-play-history__20260711.json` and the vault.

Product planning and long-form game knowledge stay in the Obsidian vault:

- `/Users/aether/obsidian/notes/Projects/RuptureOps/RuptureOps.md`
- `/Users/aether/obsidian/notes/Topics/StarRupture/StarRupture.md`

## Data versions

The corpus deliberately keeps two independent version axes:

1. the immutable StarRupture game build a fact applies to;
2. the immutable source snapshot in which a value was captured.

Do not silently treat values claimed for the Update 1 launch snapshot as values
verified against a later hotfix executable. Hidden, prototype, and play-test
content must remain excluded from player-facing defaults unless separately
verified.

Curated assumptions remain separate from raw source facts. The imported Ore
Excavator v2 purity associations and food-effect values carry explicit
confidence/provenance and can be revised without rewriting the captured source.

## Local data workflow

Use Homebrew Python 3.11 or newer on Nyx:

```bash
/opt/homebrew/bin/python3 scripts/build_database.py
/opt/homebrew/bin/python3 scripts/query.py versions
/opt/homebrew/bin/python3 scripts/query.py stats
```

The complete local corpus, including ignored raw archives, can be validated
with:

```bash
/opt/homebrew/bin/python3 scripts/validate.py
```

The raw source mirror is intentionally not committed. `starrupture.tools`
permits crawling but publishes no explicit reuse license, and the SQLite file
is reproducible from the locally tracked normalized snapshots and curated
sources. The normalized corpus is also not cleared for public redistribution.

## Status

- Local repository: `/Users/Shared/projects/ruptureops`
- Default branch: `main`
- iOS app: planning/bootstrap stage
- Current corpus: Early Access Update 1, with the latest captured source and
  public-build boundaries recorded in `data/current.json` and each snapshot
  manifest

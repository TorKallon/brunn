# StarRupture data corpus and modeling

Vault context: [RuptureOps project](</Users/aether/obsidian/notes/Projects/RuptureOps/RuptureOps.md>), [StarRupture topic](</Users/aether/obsidian/notes/Topics/StarRupture/StarRupture.md>), [Update 1 research](</Users/aether/obsidian/notes/Topics/StarRupture/Early Access - Update 1.md>)

## Purpose

Operating guide for the versioned StarRupture data corpus, SQLite knowledge base, and scenario tooling.

## Design principles

### Two-axis versioning

Every captured fact belongs to a source snapshot. Build-specific curated facts can additionally name a game-build validity range.

```text
game build axis       ea launch -> Update 1 -> 0.2.1 ... 0.2.8
source snapshot axis  SRDB version + site data claim + retrieval timestamp
```

This prevents three bad merges:

- an April 9 Update 1 number silently becoming a 0.2.8 number;
- a July source correction overwriting what the April game-data snapshot previously said;
- hidden/PTB/test data leaking into player-facing production plans.

### Raw before interpretation

The capture retains public RSC payloads, response metadata, byte counts, and SHA-256 hashes. Normalized JSON and SQLite can be rebuilt from retained source material.

Raw IDs, spelling, sign conventions, hidden state, and unresolved references are preserved. Aliases and provisional derived values live in separate fields/tables.

### Exact values versus mechanic claims

- SRDB is the designated data source for entities, recipes, buildings, unlocks, exports, and map data.
- Creepy Jar announcements are primary for Update and hotfix intent.
- Steam build IDs make release states reproducible.
- A current source capture does not reconstruct an older numeric baseline unless an older snapshot exists.

## Current snapshot

Pointer: [current.json](</Users/Shared/projects/ruptureops/data/current.json>)

Current immutable snapshot:

```text
ea-u1-gv35118973__srdb-2.3.4__20260711T050230Z
```

Components:

| Component | Value |
|---|---|
| Game-data claim | Update 1, internal game version `35118973`, dated 2026-04-09 |
| Contextual Steam mapping | Build `22674441`, inferred `0.2.0` |
| Source app | SRDB `2.3.4`, updated 2026-07-10 |
| Retrieval | 2026-07-11 05:02:30 UTC |
| Latest public build observed | `0.2.8`, Steam build `23761620` |
| Next.js build | `6APlQlwiyfFXj-ICIUtr5` |
| Image version | `1775754775` |

## Files

```text
ruptureops/
  README.md
  docs/
    data-corpus.md
  data/
    current.json
    star-rupture.sqlite3
    snapshots/<immutable snapshot>/
      manifest.json
      raw/
        site-rsc-and-static.zip
        assets.zip
        asset-fetch-manifest.json.gz
        hub-props.json.gz
        steam-news.json.gz
        crafting-oracles.json.gz
      normalized/
        items.json
        item-locations.json
        buildings.json
        recipes.json
        research.json
        corporations.json
        exports.json
        upgrades.json
        analyses.json
        experience.json
        map.json
        weapons.json
        guides.json
        tracker.json
        steam-announcements.json
        crafting-oracles.json
        asset-manifest.json
  sources/
    game-builds.json
    mechanics.json
    aliases.json
  scripts/
    refresh.py
    mirror_assets.py
    build_database.py
    query.py
    validate.py
    sr_common.py
```

The asset manifest preserves all 1,081 original references and their canonicalization. Available binaries are mirrored privately in the content-addressed `raw/assets.zip`; retrieval status, hashes, aliases, and source-dead references remain indexed per URL.

## Acquisition

The capture seeds the site hubs, reads `robots.txt` and the sitemap, and uses `/api/search` to guarantee all 468 item and 218 building detail routes are included. It then requests every indexed page with the public `RSC: 1` interface.

At the initial capture:

- 745 RSC routes requested;
- 745 captured;
- zero failures;
- 752 archived static/RSC members after adding static metadata sources.
- 1,081 original asset references reduced to 491 file-like requests plus 19 references that are not files;
- 457 direct asset downloads and 9 exact-basename resolutions, representing 466 available logical assets and 454 unique blobs;
- 25 genuine source 404s retained in the failure ledger rather than hidden.

The crawl includes 27 hidden building records that do not appear in the 191-row public building listing.

## Refresh workflow

Python 3.11 or newer is required. On this machine use the Homebrew Python explicitly because `/usr/bin/python3` is older:

```bash
cd /Users/Shared/projects/ruptureops/scripts
/opt/homebrew/bin/python3 refresh.py
/opt/homebrew/bin/python3 validate.py
```

`refresh.py`:

1. reads source/version markers;
2. creates a new immutable snapshot ID;
3. archives raw public sources;
4. normalizes the datasets;
5. canonicalizes and privately mirrors indexed assets;
6. updates `current.json`;
7. rebuilds SQLite from every complete snapshot;
8. leaves prior snapshots intact.

Use `refresh.py --skip-assets` only for an intentional metadata-only capture. To retry the current snapshot's asset ledger without recrawling page data:

```bash
/opt/homebrew/bin/python3 mirror_assets.py
```

Rebuild SQLite without recrawling:

```bash
/opt/homebrew/bin/python3 build_database.py
```

## SQLite overview

Database: [star-rupture.sqlite3](</Users/Shared/projects/ruptureops/data/star-rupture.sqlite3>)

### Provenance and versions

| Table | Purpose |
|---|---|
| `game_builds` | Immutable Steam builds, inferred semantic labels, internal version mapping, release family. |
| `source_snapshots` | SRDB version, claimed game-data context, retrieval time, complete manifest. |
| `source_documents` | URL, source type, headers, size, SHA-256, archive member. |
| `current_state` | Movable pointer to the default source snapshot. |
| `mechanic_facts` | Curated design rules, deltas, fixes, boundaries, confidence, and game-build validity. |
| `entity_aliases` | Search/display aliases without replacing source IDs. |

Entity and relationship tables inherit game-data context through `source_snapshot_id -> source_snapshots.claimed_game_build_id`. Build-specific curated mechanics name `valid_from_game_build_id` and optionally `valid_to_game_build_id` directly.

### Items and production

| Table | Purpose |
|---|---|
| `items` | Versioned item properties and provenance. |
| `item_locations` | Item-to-map-marker occurrence links from item pages. |
| `buildings` | Costs, category, raw power/capacity values, flags, source URL. |
| `building_requirements` | Construction inputs. |
| `building_sockets` | Direction/socket-group counts. |
| `base_core_levels` / `base_core_level_requirements` | Version-pinned Base Core capacity ladder and one-time upgrade inputs recovered from the retained raw building record. |
| `recipes` | Recipe header and cycle duration. |
| `recipe_inputs` / `recipe_outputs` | Relational I/O; outputs remain many-to-many capable. |
| `recipe_buildings` | Producer association, including hidden producers. |
| `recipe_producer_overrides` | Provenance-labeled curated producer associations missing from a source building's recipe array. |
| `recipe_research_inputs` | One-time unlock inputs. |
| `research_catalog` | Recipes exposed by the research dataset. |

Useful views:

- `current_items`
- `current_buildings`
- `current_public_buildings`
- `current_recipes`
- `current_recipe_options`
- `current_player_facing_recipe_options`

### Progression

| Table | Purpose |
|---|---|
| `corporations` / `corporation_levels` | Level and reputation ladder. |
| `corporation_reward_items` / `corporation_reward_buildings` | Level rewards. |
| `export_offers` | Non-zero corporation/level item values. |
| `development_upgrades` | Development Station upgrade headers and reward building. |
| `development_upgrade_requirements` | Item/Data Point costs. |
| `development_tasks` | Context-specific tracked tasks. |
| `analyses` | Item-to-Data-Point values. |
| `xp_sources` | Movement, Combat, and Survival XP sources. |

### World and combat

| Table | Purpose |
|---|---|
| `maps` | Map/layer/section metadata and coordinate warning. |
| `map_markers` | Every source marker plus stable `marker_index`. |
| `marker_rtree` | Spatial index for proximity queries. |
| `marker_ai_types` | Raw actor/archetype tags. |
| `spawn_entries` | Spawn-preset entries and counts. |
| `loot_tables` / `loot_entries` | Marker-linked item probabilities and quantity bounds. |
| `map_areas` / `area_polygon_points` | Radiation polygons and queryable points. |
| `weapons` | Raw weapon stats and provenance label. |
| `weapon_mods` / `weapon_mod_effects` | Slot, cost, description, raw attribute modifiers. |

### Research and search

| Table | Purpose |
|---|---|
| `guides` | Searchable SRDB prose guides. |
| `steam_announcements` | Official BBCode plus normalized text. |
| `patch_changes` | Individual official announcement bullets with inferred build where supported. |
| `knowledge_fts` | FTS5 across items, buildings, recipes, corporations, markers, weapons, guides, announcements, and mechanic facts. |
| `integrity_findings` | Expected source gaps and audit failures. |
| `saved_scenarios` | Reproducible future model runs pinned to source and game versions. |
| `consumable_effect_facts` | Medium-confidence imported calories/hydration assumptions kept separate from raw item records. |

### Curated import boundary

The July 11 Star Rupture Game Analysis package is an external research import,
not a canonical source snapshot. Its complete package is preserved locally and
its manifest is tracked under
`imports/star-rupture-game-analysis__20260708T190137Z/`.

Semantic reconciliation found that its item, building, recipe, progression,
export, map, loot, and official-news data duplicate the richer July 11 corpus.
Only distinct evidence was promoted:

- six medium-confidence Ore Excavator v2 impure/pure recipe-producer links;
- medium-confidence consumable effects with an explicit provenance warning;
- a player-reported save-state model;
- player-specific planning notes and site screenshots in the vault.

The legacy reports retain their own July 8 capture boundary. They must not be
relabeled with the canonical snapshot ID until their calculations are actually
ported and rerun.

## Query CLI

### Corpus status

```bash
/opt/homebrew/bin/python3 query.py versions
/opt/homebrew/bin/python3 query.py stats
```

### Search and entity tracing

```bash
/opt/homebrew/bin/python3 query.py search 'Powerium Refinery'
/opt/homebrew/bin/python3 query.py item organic-compound
/opt/homebrew/bin/python3 query.py recipe superconductor
```

`item` joins production, downstream use, research, exports, analysis, corporation rewards, map locations, and loot references.

### Spatial lookup

```bash
/opt/homebrew/bin/python3 query.py near 3000 2200 --radius 600
/opt/homebrew/bin/python3 query.py near 3000 2200 --radius 600 --type '%Ore%'
```

### Read-only SQL

```bash
/opt/homebrew/bin/python3 query.py sql \
  "SELECT name, category, stack_size FROM current_items WHERE name LIKE '%Oil%'"
```

The CLI permits `SELECT`, `WITH`, `PRAGMA`, and `EXPLAIN` only.

### Future snapshot diffs

```bash
/opt/homebrew/bin/python3 query.py diff \
  --from <old-snapshot-id> \
  --to <new-snapshot-id> \
  --entity recipes
```

Supported normalized diffs: `items`, `buildings`, and `recipes`.

## Production model

Basic use:

```bash
/opt/homebrew/bin/python3 query.py plan organic-compound --rate 45
```

Strategies:

```bash
--strategy standard
--strategy fastest
--strategy fewest-inputs
```

Pin alternatives:

```bash
--recipe liquid-helium=liquid-helium-v2
```

Allow hidden/internal producers only for deliberate investigation:

```bash
--allow-hidden
```

Persist a reproducible run:

```bash
/opt/homebrew/bin/python3 query.py plan organic-compound --rate 45 \
  --save 'Organic Compound 45 IPM baseline' \
  --notes 'Initial Update 1 comparison'
```

### Model outputs

- selected recipe per item;
- machine and recipe association;
- exact fractional and independently rounded physical machine counts;
- recurring item demand;
- raw/extracted source rates;
- one-time research costs;
- provisional power and Base Core capacity totals;
- source snapshot and claimed build;
- `complete` or `incomplete` model status;
- unresolved production demand when a recipe exists but no eligible producer is known;
- explicit warnings and model exclusions.

### Model validation

Three representative production chains match the live SRDB crafting API exactly at the raw-material-rate level. The retained 45 Organic Compound/minute oracle resolves to:

| Raw source | Per minute |
|---|---:|
| Calcium Ore | 96.625 |
| Crude Oil | 3 |
| Goethite Ore | 0.5625 |
| Helium-3 | 88.125 |
| Sulphur Ore | 163.125 |
| Titanium Ore | 81 |
| Wolfram Ore | 45 |

The API treats Power Cell as a terminal/story item while the data corpus also exposes an associated recipe. That disagreement is retained as an oracle boundary instead of forcing one interpretation to win.

The raw source has 48 recipes without a building association. Six imported,
medium-confidence Ore Excavator v2 producer overrides resolve the purity
variants, leaving 42 unresolved recipes in the active database. If one enters a
plan, the model returns `status: incomplete`, records the rate in
`unresolved_production_per_minute`, and names the affected recipe IDs rather
than treating it as a raw material.

Saved reference run: [Organic Compound 45 IPM baseline](</Users/Shared/projects/ruptureops/models/organic-compound-45-ipm-baseline__20260711T051521Z.json>)

### Current model exclusions

- floor-plan geometry and footprint;
- Drone Rail path, priority, and throughput constraints;
- storage buffers;
- Cargo parcel amount and transit time;
- power connectivity/islands;
- verified Base Core capacity semantics;
- Fire Wave downtime;
- global alternate-recipe optimization;
- progression/unlock critical path;
- loot expected value;
- verified live combat statistics.

## Validation

Run:

```bash
/opt/homebrew/bin/python3 validate.py
```

Validation currently proves:

- every archived member exists and matches its recorded size and SHA-256;
- the external Star Rupture Game Analysis ZIP matches its import ledger, passes
  ZIP integrity, and all 115 manifested files match both the package manifest
  and checksum ledger;
- all three user-authored prompt-history sources match their tracked hashes,
  byte/line/word/delimiter counts, semantic audit, and play-history source
  pointers;
- supplemental crafting-oracle and asset-archive hashes match;
- all 1,081 original asset references remain auditable, and each available content-addressed asset matches its recorded size and SHA-256;
- all canonical dataset counts match the source inventory;
- SQLite integrity and foreign keys pass;
- repeated map marker IDs are preserved with unique occurrence indices;
- four official Update 1 recipe fingerprints are present:
  - Nanofibre includes Wolfram Wire;
  - Pressure Tank excludes Titanium Housing;
  - Condenser excludes Synthetic Resin;
  - Superconductor includes Synthetic Resin and excludes Ceramics;
- three compatible production chains match the live crafting API exactly;
- the Power Cell terminal/recipe disagreement is recorded as a boundary.
- a recipe without an eligible producer is reported as unresolved production, never as a raw input.

Expected integrity findings are not hidden:

- 48 recipes have no raw-source producing-building association after all 218 building pages are crawled; six curated Ore Excavator v2 links leave 42 unresolved in the active database;
- all four weapon records carry stale or absent Update 1 provenance because the pages label the values Play Test 2.

## Source and reuse boundary

`robots.txt` permits crawling, but the SRDB footer asserts copyright and the site publishes no explicit content/data license. This is a private, attributed research archive. Do not republish the mirrored dataset or assets without separate permission.

Primary source links:

- [SRDB](https://starrupture.tools/)
- [SRDB API search index](https://starrupture.tools/api/search)
- [SRDB crafting API example](https://starrupture.tools/api/crafting/calculate?itemId=titanium-bar&rate=60)
- [Official Steam news API](https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid=1631270&count=100&maxlength=0&format=json)
- [Official Update 1 notes](https://store.steampowered.com/news/app/1631270/view/490464385050870875)

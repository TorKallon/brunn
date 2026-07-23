Created: 2026-07-10
Updated: 2026-07-10

Related: [[Topics/StarRupture/StarRupture|StarRupture]]

## Purpose

Durable, versioned reference for StarRupture's Early Access launch baseline, Update 1 release, and the Update 1 hotfix line through 0.2.8.

Use this note to answer questions about what changed, to distinguish intentional design changes from bug fixes, and to pin future calculations to an exact game snapshot. Item, recipe, building, and production values should be joined to the versioned structured data rather than assumed from the prose here.

## Snapshot boundary

### Canonical snapshots

| Snapshot | Release date | Steam build ID | Semantic version | Version confidence | Notes |
|---|---:|---:|---|---|---|
| Early Access launch | 2026-01-06 | `21164756` | `0.1.0` | High, inferred/corroborated | Official announcement does not print `0.1.0`; the next official release is Hotfix 0.1.1. |
| Final public build before Update 1 | 2026-03-25 | `22479736` | `0.1.3` | Official | Technical hotfix disabled loading saves made on the Update 1 Public Test Branch. |
| Update 1 release | 2026-04-09 | `22674441` | `0.2.0` | High, inferred | Official announcement calls it Update 1; the next official release is Hotfix 0.2.1. |
| Latest verified Update 1 hotfix | 2026-06-17 | `23761620` | `0.2.8` | Official | Latest public Steam build verified during this research on 2026-07-10. |

The immutable version key is the Steam build ID. Semantic versions are useful labels, but `0.1.0` and `0.2.0` were not printed in the corresponding official launch announcements. Preserve that provenance rather than silently treating those two semantic labels as primary-source facts.

### Time boundary

- Early Access became publicly available on 2026-01-06. SteamDB records the store release at 13:55:34 UTC; Creepy Jar's launch announcement followed at 13:58:03 UTC.
- Creepy Jar published the Update 1 announcement at 2026-04-09 09:00:35 UTC.
- Creepy Jar's Q1 2026 corporate report independently confirms both calendar dates.
- For executable/patch behavior, "latest public Update 1" means `0.2.8`, Steam build `23761620`. Numerical SRDB data remains pinned to its separate source claim: Update 1 internal game version `35118973`, dated 2026-04-09. Do not silently relabel that snapshot as 0.2.8.
- "Update 1 release" means the un-hotfixed April 9 build `22674441`, inferred semantic version `0.2.0`.

### Confidence vocabulary

- **Official:** stated in a Creepy Jar announcement, official image, or corporate report.
- **High, inferred:** not stated directly, but strongly established by adjacent official version numbering and contemporaneous artifacts.
- **Behavioral fix:** a patch correction that establishes intended behavior; do not automatically model it as a deliberate rebalance.
- **Design delta:** an announced change to progression, recipes, access, costs, or mechanics.
- **Numerically unspecified:** Creepy Jar says a value changed but did not publish the old and new values.

## Early Access baseline

At launch, StarRupture provided the Arcadia-7 open world in single-player, hosted co-op for up to four players, and an experimental dedicated-server path. Update 1 was the first free Early Access expansion.

The baseline relevant to Update 1 included:

- The original map bounded by a radiation barrier.
- Forgotten Engine as the end of the initial progression arc and the way to obtain the Teleporter.
- Corporation-based progression without the Update 1 Development Station layer.
- Quartz Building Materials used directly in construction.
- Cargo Dispatcher/Receiver links with manually selected parcel sizes and less restrictive connection behavior.
- Duplicate blueprints remaining as duplicate blueprint records rather than being converted to Data Points.
- Walkway and Dash available before the later corporation-level gates.
- The UPP-7 Pistol as a corporation reward.
- No Powerium, Goethite, Zipline network, native wildlife, or expanded post-Forgotten-Engine regions.

The immediate pre-Update-1 public baseline was version 0.1.3, build `22479736`, not the January 6 launch build. Use `21164756` only when the question is specifically about day-one Early Access.

## Update 1 net-new content

### Map, exploration, and story

- Expanded playable map with new unlockable regions.
- Finishing the Forgotten Engine now removes the radiation border and exposes the new regions.
- New Abandoned Bases, Antennas, caves, resource locations, and factory space.
- Additional foundables, dialogues, audiologs, and datapads.
- Creepy Jar did not publish a numerical map-area increase. Do not infer square kilometers from the promotional before/after map images.

### Resources and production branches

| Resource | Extraction building | Processing building | Availability |
|---|---|---|---|
| Powerium | Oil Extractor | Refinery | New regions beyond the radiation barrier |
| Goethite | Laser Drill | Pyro Forge | New regions beyond the radiation barrier |

Creepy Jar advertised more than 40 new items and recipes but did not enumerate all of them or publish all numerical rates in the patch announcement. Use the versioned structured dataset for complete recipe and production modeling.

### Advertised new buildings

The official announcement counts nine new buildings:

1. Oil Extractor
2. Laser Drill
3. Refinery
4. Pyro Forge
5. Constructorizer
6. Facturer
7. Chemical Generator
8. Roundabout Rail Connector
9. Recycler

The Development Station and Zipline were advertised separately as new features. Do not add them to the official "nine new buildings" count without noting the different marketing categories.

### Higher-tier buildings

The six explicitly advertised higher-tier buildings were:

- Compounder v.2
- Fabricator v.2
- Furnace v.2
- Ore Excavator v.2
- Orbital Cargo Launcher v.2
- Constructorizer v.2

Official prose also names Large Personal Storage, Large Habitat, and other upgraded generator, storage, and rail variants as Development Station unlocks. The six-item list is therefore the advertised headline list, not necessarily the complete set of Development Station upgrades.

### Zipline

- A player-transport network assembled from Zipline Poles and Zipline spans.
- Similar conceptually to Drone Rails, but transports players instead of resources.
- Players can traverse either direction on the constructed line.
- Update 1 placed Zipline and Zipline Pole at Clever Robotics level 6.

### Development Station

- Interior station placed inside a Habitat.
- Adds a second progression layer for improved building variants.
- Uses Quartz Ore in the new upgrade path.
- Update 1 placed it at Moon Energy level 5.
- Hotfixes 0.2.1 and 0.2.2 corrected migration and requirement tracking; see [[#Migrated-save behavior]].

### Recycler

- Interior Habitat station.
- Converts items into Basic Building Materials and/or Intermediate Building Materials.
- Converts legacy Quartz Building Materials into Quartz Ore without loss.
- Update 1 placed it at Selenian Corp level 5.

### Wildlife

- Vulpir
- Coralion
- Skylisk

Official prose describes the new animals as generally shy and peaceful, with fleeing/hiding behavior, and notes new consumables tied to wildlife. Use game data rather than marketing prose for exact drops, health, spawn rules, or food values.

## Exact design and mechanic deltas

### Corporation progression rollback

- All Corporation Levels were reset to Level 1 at Update 1 launch.
- Previous progress was converted into Data Points.
- Players could redistribute the Data Points in Corporate Terminal to unlock the revised levels and rewards.
- Already constructed buildings stayed on the map and remained operational.
- At initial Update 1 release, players could not build more copies until the relevant unlock was regained through Corporate Terminal or Development Station.

### Migrated-save behavior

The migration rule changed after launch:

1. **Update 1 release, build `22674441`:** existing buildings remained operational, but their construction unlocks had to be regained.
2. **Hotfix 0.2.2, build `22898977`:** loading a pre-Update-1 save automatically unlocked in Development Station buildings that had previously been unlocked and built.

When modeling a migrated save, specify whether the scenario is the April 9 release build or 0.2.2 and later. Current Update 1 behavior follows the latter rule.

### Starting access moved into progression

- Walkway was moved behind corporation progression; the official reward table places Extendable Walkway at Moon Energy level 4.
- Dash was moved behind corporation progression; the official reward table places Dash at Griffits Blue Corp level 3.

### Pistol and death

- UPP-7 Pistol was removed as a corporation-level reward.
- The pistol became craftable through Equipment Upgrade Station, alongside other weapons.
- Death now loses all carried weapons, explicitly including the pistol.
- Equipment Upgrade Station moved from Griffits Blue level 4 to level 2.

### Teleporter

| Baseline | Update 1 |
|---|---|
| Obtained through the Forgotten Engine progression | Clever Robotics level 9 reward |
| Forgotten Engine required | Forgotten Engine no longer required for Teleporter |

Forgotten Engine still matters because completing it removes the radiation border and opens the new regions.

### Quartz rework

- Quartz is no longer a Building Material.
- Quartz previously spent on buildings was returned as Quartz Ore.
- Legacy Quartz Building Materials in Inventory or Personal Storage cannot be used for construction.
- Recycler converts those legacy materials into Quartz Ore without loss.
- Quartz Ore is consumed by Development Station unlocks and appears as corporation-level rewards.
- Buildings that formerly required Quartz Building Materials now consume Base Core overheat capacity.

### Cargo Dispatcher/Receiver rework

| Baseline | Update 1 |
|---|---|
| Less restrictive connection topology | One Dispatcher connects to one selected Receiver |
| Player configures parcel size | No parcel-size setting |
| Existing connections persisted | Old connections invalidated and require manual reconnection |
| Dispatch cadence depended on configuration | Dispatcher automatically packs and sends every 20 seconds |

### Blueprint changes

- Picking up an already discovered blueprint now yields a Data Points item.
- Duplicate blueprints no longer accumulate in the blueprint tab.
- Blueprints formerly located in Forgotten Engine moved to new locations.
- Hotfix 0.2.1 fixed the duplicate-substitution logic for already held blueprint state.

### Explicit recipe changes

| Recipe | Early Access baseline | Update 1 |
|---|---|---|
| Nanofibre | Two components | Wolfram Wire added as a third component |
| Pressure Tank | Included Titanium Housing | Titanium Housing removed |
| Condenser | Included Synthetic Resin | Synthetic Resin removed |
| Superconductor | Used Ceramics | Uses Synthetic Resin instead |

Creepy Jar also stated that production times and required items changed across other recipes, and that various product requirements changed, but did not publish a complete numerical diff. Mark those assertions `numerically_unspecified` unless backed by versioned game data.

## Corporation reward redistribution

The following is a transcription of Creepy Jar's five official "Before & After Update 1" images. Only changed levels are listed; unlisted levels remained visually identical in the official comparison.

### Selenian Corp

| Level | Before Update 1 | Update 1 |
|---:|---|---|
| 5 | Greater Miner LEM | Recycler |
| 6 | Helium-3 Extractor | Greater Miner LEM |
| 7 | Large Habitat | Mega Press |
| 8 | Mega Press | Quartz Ore ×30 |
| 9 | Refinery; Sulfur Extractor | Laser Drill; Pyro Forge |
| 12 | — | Constructorizer |
| 13 | — | Facturer |
| 14 | — | Quartz Ore ×70 |
| 15 | — | Quartz Ore ×100 |

Unchanged in the image: levels 1–4 and 10–11.

### Moon Energy

| Level | Before Update 1 | Update 1 |
|---:|---|---|
| 4 | Lesser Infiltrator LEM | Extendable Walkway |
| 5 | Wind Turbine v.1 | Development Station |
| 6 | Base Core Amplifier v.1 | Lesser Infiltrator LEM |
| 7 | Greater Infiltrator LEM | Base Core Amplifier v.1 |
| 8 | Solar Generator v.2 | Infiltrator LEM |
| 9 | Base Core Amplifier v.2 | Wind Turbine v.1 |
| 10 | Wind Turbine v.2 | Quartz Ore ×40 |
| 12 | — | Greater Infiltrator LEM |
| 13 | — | Chemical Generator |
| 14 | — | Quartz Ore ×70 |
| 15 | — | Quartz Ore ×100 |

Unchanged in the image: levels 1–3 and 11.

### Future Health Solutions

| Level | Before Update 1 | Update 1 |
|---:|---|---|
| 1 | Habitat; Basic Habitat Modules; Advanced Habitat Modules | Basic Habitat Modules; Advanced Habitat Modules |
| 5 | Lesser Dietician LEM; Lesser Irrigator LEM | Food Station |
| 6 | Food Station | Helium-3 Extractor |
| 8 | Dietician LEM; Irrigator LEM | Pressurizer; Sulfur Extractor |
| 10 | Resister LEM | Dietician LEM; Irrigator LEM |
| 11 | Greater Irrigator LEM; Greater Dietician LEM | Oil Extractor; Refinery |
| 12 | — | Greater Irrigator LEM; Greater Dietician LEM |
| 13 | — | Quartz Ore ×70 |
| 14 | — | Quartz Ore ×100 |

Unchanged in the image: levels 2–4, 7, and 9.

### Clever Robotics

| Level | Before Update 1 | Update 1 |
|---:|---|---|
| 1 | Corporate Terminal; Orbital Cargo Launcher; Rail v.1; Rail Support; Rail Connector | Corporate Terminal; Rail v.1; Rail Support; Rail Connector |
| 6 | Rail v.2 | Zipline Pole; Zipline |
| 8 | Storage Depot v.2 | Roundabout Rail Connector |
| 9 | Large Personal Storage | Teleporter |
| 11 | Rail v.3 | Quartz Ore ×50 |
| 13 | Expandable Storage | Quartz Ore ×70 |
| 14 | — | Quartz Ore ×100 |

Unchanged in the image: levels 2–5, 7, 10, and 12.

### Griffits Blue Corp

| Level | Before Update 1 | Update 1 |
|---:|---|---|
| 1 | Basic Platforms; Advanced Platforms; Platform Modules | Orbital Cargo Launcher; Basic Platforms; Advanced Platforms; Platform Modules |
| 2 | UPP-7 Pistol | Equipment Upgrade Station |
| 3 | Grenade | Dash |
| 4 | Equipment Upgrade Station | Grenade |
| 11 | Greater Shieldgiver LEM; Greater Lifegiver LEM | Quartz Ore ×50 |
| 12 | — | Greater Shieldgiver LEM; Greater Lifegiver LEM |
| 13 | — | Quartz Ore ×70 |
| 14 | — | Quartz Ore ×100 |

Unchanged in the image: levels 5–10.

The comparison images identify reward movement but do not identify the final destination of every removed v.2, storage, rail, Habitat, or LEM reward. Many became Development Station unlocks, but do not infer an exact destination without structured game data.

## Mechanically relevant Update 1 fixes

These changes took effect in the Update 1 release build but are primarily corrections of intended behavior. Store them as `bugfix_behavior` unless other evidence identifies a deliberate rebalance.

### Production and logistics

- Fixed production stalls when multiple independent Drone Rail lines merged into one destination.
- Stopped Drones from teleporting between lanes through Multirail 3 and Multirail 5.
- Added direction indicators to Drone junctions.
- Upgrading Drone Rails now returns products present on the rail.
- Universal Storage now prioritizes high-demand requests correctly.
- Fixed Dispatcher/Receiver communication with Universal Storage.
- Manually filling a building now clears obsolete drone requests.
- Improved Storage Depot product send-out behavior.
- Orbital Cargo Launcher now retains selected Corporation and shipment progress across reloads.
- Orbital Cargo Launcher continues requesting items when a Corporation reaches maximum level.

### Building, stability, and power

- Connecting Platforms can snap to Pillars and Habitat roofs.
- Platforms can be placed on gravel.
- Duplicate Platform placement on water and placement in restricted regions were blocked.
- Habitat and Large Habitat now correctly consume Base Core overheat capacity.
- Habitat interiors now correctly consume Building Materials.
- Platform Energy Drain at 100% usage was corrected.
- Various foundation, snapping, stability, Airlock, Ladder, Stair, and refund behaviors were corrected.

### Combat, enemies, and environment

- Forgotten Engine enemy damage increased.
- Goliath turning speed toward the player reduced.
- Threatened and non-threatened enemy movement speeds adjusted.
- Encounter spawn rate adjusted.
- Swarm pathfinding and targeting corrected.
- Flinger projectile redirection, Slasher grenade reactions, Spitter damage, and Exploder timing/hitboxes corrected.
- Excessive experience from SLAMS-12 kills corrected.
- Fire Wave damage scaling corrected.
- Infection behavior around Antennas and production buildings corrected.

### Player and interface behavior

- Added autosave on/off control.
- Movement can be bound to arrow keys or mouse buttons.
- Directional movement speed improved.
- Double-click item transfer behavior improved.
- Local/Steam Cloud save conflict handling improved.

## Public Test Branch caveats

### PTB timeline

| Date | Channel | Build | Meaning |
|---|---|---:|---|
| 2026-03-25 | `public_test_branch` | `22481363` | Initial Update 1 pre-release build |
| 2026-04-01 | `public_test_branch` | `22581203` | PTB Hotfix 1 |
| 2026-04-09 | `public` | `22674441` | Final Update 1 release |

- PTB builds were explicitly works in progress.
- Saves written by the PTB were incompatible with the standard build.
- Public Hotfix 0.1.3 disabled loading PTB saves.
- The PTB closed when Update 1 launched.
- Do not mix PTB observations into the public Update 1 snapshot without a release-build confirmation.

### PTB Hotfix 1 balance changes

The following adjustments fed into the final release, but Creepy Jar did not publish numerical before/after values:

- Build costs of Power Generators, Zipline, Drone Rails, and Teleporter.
- Energy consumption and Base Core cooling for v.2 buildings.
- Energy production and Base Core cooling for Power Generators.
- Scanner unlock requirements.
- Constructorizer and Facturer Recipe Station unlock requirements.
- `Radial Rail Connector` renamed `Roundabout Rail Connector`.

These are confirmed changes but are `numerically_unspecified`. A current Update 1 data snapshot can establish final values; it cannot by itself reconstruct the earlier PTB or 0.1.3 values.

## Hotfix lineage through 0.2.8

| Version | Date | Steam build | Material gameplay/data implications |
|---|---:|---:|---|
| Update 1 / inferred 0.2.0 | 2026-04-09 | `22674441` | Major content and mechanic release described above. |
| 0.2.1 | 2026-04-16 | `22793038` | Corrected Recipe Station requirements, duplicate-blueprint substitution, Development Station tracking, progression migration, and repeated Quartz refunds. |
| 0.2.2 | 2026-04-22 | `22898977` | Auto-unlocks prior buildings in Development Station for pre-Update-1 saves; improves long-distance Drone pulling and Cargo Dispatcher progress; corrects Turret targeting and Grenade damage distribution; adjusts Vulpir/Coralion egg spawning. |
| 0.2.3 | 2026-05-04 | `23072974` | Standard Ammo recipe now unlocks with the Turret reward; building/stability fixes; improved Grubbler spawning. |
| 0.2.4 | 2026-05-06 | `23106997` | Crash-only hotfix. |
| Unannounced build | 2026-05-14 | `23229401` | Public build with no official patch notes. Version label unknown; preserve as its own build rather than folding silently into 0.2.4. |
| 0.2.5 | 2026-05-18 | `23282460` | Adds autosave interval; decreases Platform stability cost; improves Habitat stability; adjusts regional Goliath spawning, Skylisk collision, and foundable interaction range. |
| 0.2.6 | 2026-06-02 | `23504314` | Stability/save fixes, including correction of Habitat stability state on saves created before 0.2.5. |
| 0.2.7 | 2026-06-11 | `23654858` | Crash fix, co-op dialogue correction, and Crash Report Tool improvement. |
| 0.2.8 | 2026-06-17 | `23761620` | Fixes exit crash on NVIDIA RTX 40- and 50-series GPUs. |

### Current Update 1 mechanics that differ from release day

For scenario modeling on the latest Update 1 build, carry forward at least these post-launch changes:

- Migrated pre-Update-1 building unlocks are restored automatically in Development Station from 0.2.2 onward.
- Standard Ammo unlock accompanies the Turret reward from 0.2.3 onward.
- Platform stability cost is lower from 0.2.5 onward.
- Autosave interval is configurable from 0.2.5 onward.
- Wildlife, enemy spawn, Turret targeting, Grenade distribution, and long-distance Drone behavior include the later corrections listed above.

## Aliases and data-quality notes

| Observed term | Canonical/normalized term | Note |
|---|---|---|
| Radial Rail Connector | Roundabout Rail Connector | Official PTB rename. |
| Teleport | Teleporter | `Teleport` appears in the PTB cost-adjustment note. |
| Corallion | Coralion | `Corallion` appears once in official release prose; repeated official lists and hotfixes use `Coralion`. |
| Larger Personal Storage | Large Personal Storage | Prose/image terminology differs. Preserve alias. |
| Roundabout Rail Connector | Roundabout Rail Connector | Some official material abbreviates this to Roundabout Connector or groups it under Junctions. |

Additional quality cautions:

- The official before/after Selenian image lists `Refinery` before Update 1 even though launch marketing calls Refinery new. Preserve both primary-source facts; do not use the marketing list alone as an entity-introduction ledger.
- Higher-tier headline counts and Development Station contents are not equivalent lists.
- Official patch notes contain broad phrases such as "changed various requirements" and "rebalanced various recipes." Those statements prove a change occurred but do not establish a numerical delta.
- A current `starrupture.tools` snapshot is authoritative for the site's present dataset, not automatically for April 9 or the January baseline. Every imported row should carry source-capture and game-version metadata.
- Steam announcement GIDs differ from the store-news article IDs. Preserve both when archiving raw source documents.

## Query guidance

When answering or modeling:

1. Resolve the requested game snapshot to a Steam build ID.
2. Use item/recipe/building rows valid for that snapshot.
3. Apply progression rules separately from production recipes.
4. Distinguish a fresh save from a migrated pre-Update-1 save.
5. Treat PTB-only values as a separate release channel.
6. Mark unspecified numeric changes rather than inventing values.
7. For "Update 1" without further qualification, use 0.2.8/build `23761620` for known patch behavior, use the named SRDB snapshot for numerical data, and disclose the boundary.

Suggested provenance fields for structured facts:

```text
valid_from_build
valid_to_build
semantic_version
semantic_version_confidence
release_channel
change_kind
source_url
source_capture_date
source_record_id
confidence
notes
```

Recommended `change_kind` values:

```text
content_addition
design_delta
balance_delta
bugfix_behavior
migration_rule
rename
numerically_unspecified
```

## Source manifest

### Primary sources

- [StarRupture Early Access Available Now — official Steam community announcement](https://steamcommunity.com/games/1631270/announcements/detail/502844844914249884)
- [StarRupture Early Access Roadmap — official Steam community announcement](https://steamcommunity.com/games/1631270/announcements/detail/502846747199931098)
- [Update 1 Public Test Branch announcement](https://store.steampowered.com/news/app/1631270/view/506228251914404972)
- [Update 1 PTB Hotfix 1](https://steamcommunity.com/games/1631270/announcements/detail/542260850095816982)
- [Update 1 announcement / preview](https://store.steampowered.com/news/app/1631270/view/503976001435336748)
- [Update 1 now available — full official notes](https://store.steampowered.com/news/app/1631270/view/490464385050870875)
- [Update 1 Changes Overview](https://store.steampowered.com/news/app/1631270/view/541135584198920980)
- [Hotfix 0.2.1 official announcement](https://steamcommunity.com/games/1631270/announcements/detail/541135584198923068)
- [Hotfix 0.2.2 official announcement](https://steamcommunity.com/games/1631270/announcements/detail/496100856889868383)
- [Hotfix 0.2.3 official announcement](https://steamcommunity.com/games/1631270/announcements/detail/694259874864824745)
- [Hotfix 0.2.4 official announcement](https://steamcommunity.com/games/1631270/announcements/detail/694260508207874076)
- [Hotfix 0.2.5 official announcement](https://steamcommunity.com/games/1631270/announcements/detail/659358246189924379)
- [Hotfix 0.2.6 official announcement](https://steamcommunity.com/games/1631270/announcements/detail/714528609564363488)
- [Hotfix 0.2.7 official announcement](https://steamcommunity.com/games/1631270/announcements/detail/715655143637386582)
- [Hotfix 0.2.8 official announcement](https://steamcommunity.com/games/1631270/announcements/detail/671745681362258109)
- [Official Steam news API for app 1631270](https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid=1631270&count=100&maxlength=0&format=json)
- [Creepy Jar Q1 2026 report](https://creepyjar.com/wp-content/uploads/2026/05/Creepy-Jar-Report-1Q2026_FIN_EN.pdf)

### Official reward comparison images

- [Selenian Corp before/after](https://clan.cloudflare.steamstatic.com/images/44238153/f8ade66898c04e64729a446da82fb2bd15894040.png)
- [Moon Energy before/after](https://clan.cloudflare.steamstatic.com/images/44238153/7be00f4e8fe918fb7a6cc122872ca4fc7e48c67f.png)
- [Future Health Solutions before/after](https://clan.cloudflare.steamstatic.com/images/44238153/3e950fe68b757b86fe9490ae720da82aecfa2113.png)
- [Clever Robotics before/after](https://clan.cloudflare.steamstatic.com/images/44238153/0036d3f69e23bad3be774e13fe13b6ad7b7bde25.png)
- [Griffits Blue Corp before/after](https://clan.cloudflare.steamstatic.com/images/44238153/689bca66b480015871ff95da72cc72f4310eadf1.png)

### Build metadata and patch mirrors

SteamDB is secondary rather than an official Creepy Jar source, but it provides the build identifiers needed to make snapshots reproducible.

- [Early Access launch — build 21164756](https://steamdb.info/patchnotes/21164756/)
- [Hotfix 0.1.3 technical build — 22479736](https://steamdb.info/patchnotes/22479736/)
- [PTB initial build — 22481363](https://steamdb.info/patchnotes/22481363/)
- [PTB Hotfix 1 — 22581203](https://steamdb.info/patchnotes/22581203/)
- [Update 1 release — build 22674441](https://steamdb.info/patchnotes/22674441/)
- [Hotfix 0.2.1 — build 22793038](https://steamdb.info/patchnotes/22793038/)
- [Hotfix 0.2.2 — build 22898977](https://steamdb.info/patchnotes/22898977/)
- [Hotfix 0.2.3 — build 23072974](https://steamdb.info/patchnotes/23072974/)
- [Hotfix 0.2.4 — build 23106997](https://steamdb.info/patchnotes/23106997/)
- [Unannounced public build — 23229401](https://steamdb.info/patchnotes/23229401/)
- [Hotfix 0.2.5 — build 23282460](https://steamdb.info/patchnotes/23282460/)
- [Hotfix 0.2.6 — build 23504314](https://steamdb.info/patchnotes/23504314/)
- [Hotfix 0.2.7 — build 23654858](https://steamdb.info/patchnotes/23654858/)
- [Hotfix 0.2.8 — build 23761620](https://steamdb.info/patchnotes/23761620/)
- [StarRupture complete patch history](https://steamdb.info/app/1631270/patchnotes/)

## Research limitations

- Official material does not provide a complete historical numerical export for recipes, rates, costs, energy, cooling, or unlock requirements.
- The PTB adjustments are qualitative only.
- The exact semantic version shown inside build `22674441` was not captured from a primary source during this research; `0.2.0` remains a high-confidence inference.
- The May 14 public build `23229401` has no official notes and should remain an explicit unknown in the lineage.
- Future updates may change `starrupture.tools`; preserve raw captures and content hashes rather than relying on live URLs alone.

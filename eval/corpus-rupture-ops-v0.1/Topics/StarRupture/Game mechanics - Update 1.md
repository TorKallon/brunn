Created: 2026-07-10
Updated: 2026-07-10

Related: [[Topics/StarRupture/StarRupture|StarRupture]], [[Topics/StarRupture/Early Access - Update 1|Early Access - Update 1]], [[Topics/StarRupture/Knowledge base and modeling|Knowledge base and modeling]]

## Purpose

System map for reasoning about StarRupture during Early Access Update 1.

This note identifies the mechanics that must stay separate, how they connect, which parts are already represented numerically, and where the evidence is still incomplete. Exact entity values belong in the versioned database; this note preserves interpretation and modeling boundaries.

## Core loop

At a high level:

1. Explore Arcadia-7 and reveal terrain, deposits, points of interest, foundables, blueprints, and story gates.
2. Extract solid, gas, liquid, cave, wildlife, and limited-event resources.
3. Process resources through machine-specific recipes.
4. Move and buffer products through Drone Rails, storage, and paired Cargo Dispatcher/Receiver links.
5. Power the electrical network and stay within Base Core capacity/overheat limits.
6. Analyze valuables, research recipes, export products, level corporations, complete Development Station tasks, and advance player/equipment/story progression.
7. Protect the player and factory from hostile creatures, infection, environmental hazards, and the Ruptura/Fire Wave cycle.

The useful modeling insight is that this is not one tech tree. Several progression currencies and gates cross the same production graph.

## Progression systems

### Corporation progression

Corporations have levels, reputation thresholds, and item/building rewards. The captured data includes five live specialist corporations plus the Training Corporation:

- Clever Robotics — logistics and transport.
- Future Health Solutions — survival, medical, and biological production.
- Griffits Blue Corp — military platforms, equipment, weapons, and combat rewards.
- Moon Energy — power, Base Core, traversal, and Development Station progression.
- Selenian Corporation — mining, extraction, and industrial production.
- Training Corporation — tutorial/internal progression content.

Update 1 reset corporation levels, converted prior progress to Data Points, redistributed rewards, and added higher levels. See [[Topics/StarRupture/Early Access - Update 1#Corporation reward redistribution|Corporation reward redistribution]].

Do not equate:

- Data Points allocated to corporation progression;
- corporation reputation thresholds;
- export values;
- player XP;
- recipe research costs;
- Development Station requirements;
- equipment currency.

### Recipe research

The Recipe Station unlock path is represented by 137 researchable recipes and 469 material-input rows in the captured source. A research requirement can include:

- production items;
- a found blueprint;
- Data Points;
- large one-time quantities that are not recurring factory throughput.

The production model reports one-time research costs separately from items-per-minute demand.

### Blueprint acquisition

Blueprints are found in the world and unlock recipes. Update 1 changed duplicate pickups into a Data Points item and relocated blueprints formerly found in the Forgotten Engine.

Blueprint state therefore connects:

```text
world location -> pickup state -> recipe research -> production availability
                     |
                     +-> duplicate pickup -> Data Points item
```

### Development Station

The Development Station is a separate building-upgrade system. The captured data includes 15 upgrades, 45 item requirements, and 32 tracked tasks.

An upgrade can depend on:

- corporation identity and level;
- reputation threshold;
- item and Data Point costs;
- context-specific tasks such as owning a Base Core level, unlocking recipes, constructing buildings, taking loot, laying rail, or meeting power conditions;
- a higher-tier building reward.

Do not model a Development Station upgrade as a normal production recipe.

### Player XP

The captured XP dataset has 25 sources classified as:

- Combat;
- Movement;
- Survival.

Enemy kills, movement actions, and survival actions can therefore advance a player system independently of factory progression. Exact level thresholds are not present in the current high-yield dataset and remain a gap.

### Equipment progression

The Equipment Upgrade Station owns weapon crafting and modification. Update 1 moved the pistol here from corporation rewards. Weapon modifications carry a slot, equipment-currency cost, and one or more raw attribute modifiers.

The current SRDB weapon numbers are still labeled Play Test 2. They are indexed for investigation but not treated as verified Update 1 combat balance.

### Base Core and story progression

Base Core levels and amplifiers govern factory capacity/protection. Story progress through the Forgotten Engine removes the radiation border and opens the Update 1 regions.

These gates interact but remain distinct:

- Teleporter is a Clever Robotics reward in Update 1.
- Forgotten Engine is no longer the Teleporter gate.
- Forgotten Engine still gates the expanded regions by removing the radiation boundary.

## World and exploration

### Map representation

The snapshot preserves:

- 3,259 map markers;
- 1,384 marker-linked loot tables;
- 3,094 loot entries;
- 1,063 NPC spawn markers and raw spawn presets;
- resource nodes with purity in their marker type;
- caves, Antennas, Monoliths, abandoned bases, dead bodies, rubble, loot boxes, wrecks, and abandoned machines;
- two radiation polygons.

Marker IDs are not unique. The database preserves source list position as `marker_index`, so repeated IDs are occurrences rather than accidental duplicates to delete.

### Coordinate boundary

SRDB coordinates are indexed spatially with SQLite RTree. They are valid for proximity searches inside this captured map space.

Do not combine them with another map, raw Unreal world coordinates, screenshots, or route-planner coordinates until a transform is verified.

### Points of interest and discoverables

Relevant POI classes include:

- Antennas / Geo Scanners;
- Monoliths / Obelisks;
- caves and Cave Hearts;
- abandoned bases;
- Forgotten Engine;
- foundables, audiologs, datapads, story items, and blueprints;
- abandoned storage and production machines.

Static markers do not fully express conditional story, Fire Wave, infection, or already-looted state.

### Traversal

Traversal mechanics include walking, sprinting, sliding, jumping, double-jumping, Dash, Zipline, and Teleporter.

Update 1 specifics:

- Dash moved behind Griffits Blue level 3.
- Zipline and Zipline Pole became Clever Robotics level 6 rewards.
- A Zipline graph permits travel in either direction.
- Teleporter became Clever Robotics level 9 rather than a Forgotten Engine reward.

## Environment and survival cycle

### Ruptura / Fire Wave

The environmental cycle affects the player, world resources, factories, building heat/protection, and hostile encounters.

The SRDB 2.3.4 timer tool models:

| Phase | Seconds |
|---|---:|
| Burning | 30 |
| Cooling | 60 |
| Stabilizing | 600 |
| Stable | 2,550 |
| Total | 3,240 / 54 minutes |

This is community-tool logic, not an official guaranteed formula. Keep it pinned to the SRDB capture when planning time windows.

### Regeneration

Official Update 1 prose confirms that foliage regrows and water replenishes after the Fire Wave. The broader regeneration model may also affect gatherables, wildlife, eggs, caves, and encounter state, but exact timing and formulas are not published in the retained primary sources.

### Hazards and status systems

Mechanically distinct hazards include:

- Fire Wave heat and damage;
- radiation and boundary access;
- Sulfur-cloud corrosion;
- infection clouds and cysts;
- toxicity;
- fall damage;
- hostile creature attacks;
- Base Core/factory overheat consequences.

Consumable descriptions, LEM descriptions, and patch notes expose parts of these systems. Many exact status durations, resistances, and stacking rules remain unverified.

### Shelter and protection

Habitats and Base Core protection interact with the Fire Wave. Base Core capacity/overheat is also a construction constraint, especially after former Quartz-cost buildings moved into that budget.

Do not treat electrical power and Base Core capacity as the same network.

## Resources, crafting, and production

### Resource identity

Separate the deposit from the item it produces. Important examples:

| Deposit/mechanic | Extracted item | Building |
|---|---|---|
| Titanium, Wolfram, Calcium with Impure/Normal/Pure nodes | Corresponding ore | Ore Excavator variants / Laser Drill variants |
| Helium-3 deposit | Helium-3 | Helium-3 Extractor |
| Sulfur deposit | Sulphur Ore | Sulfur Extractor |
| Goethite deposit | Goethite Ore | Laser Drill |
| Powerium deposit | Crude Oil (`magic-oil-ore`) | Oil Extractor |

Powerium is not simply a display alias for every downstream oil product.

### Recipe model

Every retained recipe has:

- raw recipe ID;
- level;
- cycle duration;
- zero or more inputs and quantities;
- an output and quantity;
- zero or more producing-building associations;
- optional one-time research inputs.

The current source uses one output per recipe, but the SQLite schema keeps outputs relational so future multi-output recipes do not require a redesign.

### Throughput formulas

For a timed recipe:

```text
output_per_minute = output_quantity * 60 / duration_seconds
fractional_machines = target_output_per_minute / output_per_minute
input_per_minute = target_output_per_minute * input_quantity / output_quantity
```

The model reports:

- exact fractional machine demand;
- an independent ceiling count for physical machines;
- recurring upstream rates;
- zero-input extraction/passive sources;
- one-time research costs;
- provisional power and capacity totals.

Ceiling overproduction is not yet propagated back through the entire graph.

### Processing stages

The production graph spans:

- deposit extraction;
- ore, gas, and liquid processing;
- smelting, Furnace, Refinery, and Pyro Forge steps;
- Compounder chemistry;
- Fabricator manufacturing;
- Assembler products;
- Constructorizer and Facturer advanced production;
- personal/item-printer crafting;
- food and medical production;
- recycling;
- Orbital Cargo Launcher exports.

### Alternate recipes and higher tiers

An item may have multiple recipe options, including v.2 or purity-specific extraction recipes. The query tool supports:

- `standard` — prefer conventional non-v.2 recipes;
- `fastest` — prefer maximum output rate per producer;
- `fewest-inputs` — prefer fewer and smaller immediate inputs;
- explicit `ITEM=RECIPE` overrides.

These are heuristics, not a global optimizer. They do not yet minimize total raw resources, power, rail load, unlock cost, or floor area.

## Construction and base infrastructure

### Placement and structure

Building is constrained by more than material cost:

- Base Core range and capacity;
- terrain and restricted-area rules;
- deposit-specific placement;
- Habitat-interior-only stations;
- snapping, platform, pillar, roof, Airlock, and connector relationships;
- structural stability and supported platforms;
- dependent deconstruction;
- Fire Wave and infection state.

Hotfix 0.2.5 lowered Platform stability cost, so stability-sensitive scenarios must distinguish 0.2.0–0.2.4 from 0.2.5 and later.

### Construction costs

All 218 indexed building detail pages were captured, including 27 hidden records. Construction requirements, raw power, raw temperature/capacity, sockets, hidden state, and recipes are normalized.

Hidden, test, custom, non-deconstructible, foundation, and internal variants remain in the corpus. Default production models require a public, non-hidden producer.

## Power and Base Core capacity

Model these as two separate constraints.

### Electrical power

SRDB raw sign convention:

- positive `power_raw` appears to mean generation;
- negative `power_raw` appears to mean consumption.

The database derives generation and demand without deleting `power_raw`.

Electrical topology can involve generators, consumers, rails, platforms, bridges, connected components, saturation, and Energy Drain. The current recursive model sums machines but does not prove connectivity.

### Base Core capacity / temperature

SRDB values suggest:

- positive `temperature_raw` is capacity load;
- negative `temperature_raw` is a capacity contribution/bonus.

The descriptions and numbers are not uniformly self-consistent. Derived load and bonus totals are provisional and always accompanied by the raw values.

A future capacity model needs Base Core level curves, amplifier bonuses, protection radius/state, Fire Wave consequences, and validation against the live game.

## Logistics and transport

### Drone Rails

Drone logistics are a graph, not a flat items-per-minute number. Preserve:

- rail tier and nominal throughput;
- socket groups and direction counts;
- poles, Multirails, junctions, modulators, and Roundabout connectors;
- building input/output sockets;
- request and supply behavior;
- storage priority and overflow;
- Smart Path behavior;
- power conduction;
- in-place rail upgrades and item refunds.

The current database has the components required to construct this graph, but path selection and priority formulas are not yet implemented.

### Storage

Storage systems differ in capacity, accepted item behavior, request/supply behavior, and overflow role. Universal Storage priority and long-distance drone pulling have received behavioral fixes during Update 1, so patch lineage matters.

### Cargo

Update 1 makes Cargo links one Dispatcher to one Receiver and dispatches automatically every 20 seconds. A future cargo model needs buffer capacity, packing amount, transit latency, and failure/reconnect behavior.

### Orbital exports

Orbital Cargo Launcher shipments connect production to corporation-specific values and progression. The same item can have different values at different levels and corporations.

## Player, inventory, and equipment

Player systems include:

- health;
- calories;
- hydration;
- toxicity;
- stamina and movement costs;
- inventory slots and item stacks;
- tools, weapons, ammunition, grenades, and corpse recovery;
- Combat, Movement, and Survival LEMs;
- consumable effects and tradeoffs.

Update 1 makes all carried weapons losable on death. The current item dataset indexes descriptions and stacks, but many consumable numeric effects are blank in SRDB 2.3.4; do not fill those gaps from stale search snippets without a versioned source.

## Combat, enemies, and ecology

### Weapons

Indexed weapon fields include:

- ammo type;
- magazine size;
- firing mode;
- damage;
- raw fire-rate value;
- range;
- shots;
- spread;
- reload time;
- modification slots, costs, and raw modifiers.

The Play Test 2 provenance warning prevents these values from being treated as confirmed Update 1 DPS inputs.

### Hostile actors and infection

Map spawn presets retain raw AI archetypes, tiers, custom names, spawn-count values, burst conditions, and other source JSON. Known high-level families include Slashers, Spitters, Flingers, Goliaths, Exploders, and swarmer/cluster behaviors.

Enemy health, damage, armor/resistance, attack timing, infection probability, and exact display-name mapping are incomplete.

### Player-observed Geo Scanner failure chain

The player's first completed Geo Scanner defense provides direct Update 1 field evidence for an important recovery interaction:

- physically installed but unpowered turrets did not defend the scanner fire base;
- after the player was overwhelmed, infection/corruption disabled that base and its claimed Regeneration Chamber;
- because the intended local chamber was offline, the player respawned at the starting launcher;
- a later attempt succeeded after the temporary fire base was fully built out and every turret was powered.

This establishes the observed sequence for that encounter, not a universal corruption threshold or a complete formula for Regeneration Chamber availability. Operationally, scanner plans should keep a replacement-gear cache outside the contested base's failure domain and verify turret power/firing before activation.

### Wildlife and gatherables

Update 1 adds Vulpir, Coralion, and Skylisk. Wildlife connects to spawn/regeneration, eggs, meat/consumables, analysis, and collision/behavior fixes.

Plant categories are present on the map source, but the current map payload has zero plant markers. That is a source gap, not proof that plants do not exist.

## Co-op and dedicated servers

The official model supports up to four players. Preserve synchronization scope separately for:

- host, client, and dedicated-server authority;
- map reveal;
- corporation/progression state;
- inventory and corpse recovery;
- Regeneration Chamber claims and character selection;
- construction attribution and refunds;
- session join/rejoin state;
- friendly fire and co-op LEM effects;
- invites, lobby slots, chat, and session visibility.

Patch notes document many synchronization bugs. A fix proves that the state existed and was corrected; it does not automatically reveal the complete intended ownership formula.

## Data-quality boundaries

- The source claims Update 1 game data dated 2026-04-09 even though the SRDB application was updated 2026-07-10.
- `discovered` is not a reliable availability flag.
- Hidden `false` is not sufficient to prove player availability because public lists contain custom and test entities.
- Internal IDs and display names differ; aliases do not overwrite raw values.
- Loot probability semantics are unvalidated.
- Weapon statistics retain Play Test 2 provenance.
- Patch-note fixes are not automatically design rules.
- Broad phrases such as “various recipe changes” prove change but not a numerical delta.
- Current source data cannot reconstruct pre-Update 1 numerical values by itself.

## High-value next models

The corpus is prepared for, but does not yet fully implement:

- global alternate-recipe optimization by raw materials, power, area, or unlock cost;
- corporation-level export mix optimization;
- combined power-grid and Base Core capacity planning;
- Drone Rail path and throughput validation;
- Cargo versus rail network comparison;
- deposit/base-site selection using purity, resource mix, distance, radiation, and story gates;
- blueprint and POI route planning;
- Fire Wave window and downtime planning;
- turret/ammunition demand against encounter compositions;
- weapon/mod/LEM DPS and sustain after live-stat verification;
- expected loot and Data Point return after probability validation;
- pre-Update 1 save migration audits;
- build-to-build recipe, unlock, cost, and map diffs.

## Sources

- [SRDB items](https://starrupture.tools/items)
- [SRDB buildings](https://starrupture.tools/buildings)
- [SRDB crafting](https://starrupture.tools/crafting)
- [SRDB research](https://starrupture.tools/research)
- [SRDB exports](https://starrupture.tools/exports)
- [SRDB Development upgrades](https://starrupture.tools/upgrades)
- [SRDB analysis](https://starrupture.tools/analysis)
- [SRDB experience](https://starrupture.tools/experience)
- [SRDB map](https://starrupture.tools/map)
- [Official Update 1 notes](https://store.steampowered.com/news/app/1631270/view/490464385050870875)
- [Official Update 1 changes overview](https://store.steampowered.com/news/app/1631270/view/541135584198920980)
- [Official Base Building Basics Part 2](https://store.steampowered.com/news/app/1631270/view/673998750553735383)
- [Official Steam Early Access page](https://store.steampowered.com/app/1631270/StarRupture/)

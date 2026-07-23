Created: 2026-07-07
Updated: 2026-07-07

# Star Rupture logistics

The transport system is **drone rails**: rail segments connect building sockets; drone workers (via Drone Stations) carry items along them. Rails also **conduct electricity** — the rail network doubles as the power grid, so a well-planned rail backbone distributes both items and power.

Data source: starrupture.tools crawl 2026-07-07 (see [[Star Rupture]] for source caveats). Game version: Early Access Update 1 (0.2.x).

## Rail tiers and throughput

| Rail | Throughput | Build cost | Status in Update 1 |
|---|---|---|---|
| Rail v.1 | **120 items/min** | 1 Basic Building Material | In game (Clever Robotics L1 / starting track L2) |
| Rail v.2 | **240 items/min** | 10 Basic Building Material | In game (Development Station upgrade) |
| Rail v.3 | **480 items/min** | 3 Intermediate Building Material | In game (Development Station upgrade) |
| Rail v.4 | 750 items/min | (site: 5 Intermediate Building Material) | **NOT in game yet** — future content |
| Rail v.5 | 1,500 items/min | (site data placeholder) | **NOT in game yet** — future content |

All rails: 0 power draw, conduct electricity.

## Rail network components (all in Clever Robotics' track unless noted)

| Building | In-game name | Unlock | Function |
|---|---|---|---|
| drone-rail-t1 | Rail v.1 | Clever L1 | Basic rail segment |
| drone-pole | Rail Support | Clever L1 | Supports long rail spans |
| drone-junction-4 | Rail Connector | Clever L1 | Split/expand rail paths (4-way) |
| drone-junction | Drone Junction | (base) | Split/expand rail paths |
| drone-lane-3 | Multirail 3 | Clever L5 | Carries up to 3 parallel rails on one support |
| drone-merger-3to1 | Rail Modulator 3 | Clever L5 | Merge or split up to 3 rails |
| zip-rail | Zipline | Clever L6 | Player fast travel (no electricity) |
| drone-roundabout | Roundabout Rail Connector | Clever L8 | Organized multi-way splitting — cleaner than stacking junctions |
| drone-lane-5 | Multirail 5 | Clever L10 | 5 parallel rails on one support |
| drone-merger-5to1 | Rail Modulator 5 | Clever L10 | Merge/split up to 5 rails |
| drone-station | Drone Station | (base) | Sends drone workers that do the actual hauling via rails; 5 power |

## Machine rail sockets (inputs → outputs)
Machines connect to rails through fixed sockets — this caps how many independent feeds each machine can take.

| Machine | Inputs | Outputs |
|---|---|---|
| Smelter | 1 | 1 |
| Fabricator (Crafter) | 2 | 1 |
| Fabricator v.2 | 3 | 1 |
| Furnace | 3 | 1 |
| Furnace v.2 | 4 | 1 |
| Compounder (Synthetizer) / v.2 | 3 | 1 |
| Assembler | 3 | 1 |
| Mega Press (Hammer) | 4 | 1 |
| Pyro Forge | 2 | 1 |
| Constructorizer (Factory) / v.2 | 4 | 1 |
| Facturer (Military Assembler) | 5 | 1 |
| All drills/extractors | — | 1 |
| Orbital Cargo Launcher (Exporter) | 1 | — |
| Refinery (Pressurizer) | 3 | 1 |

## Storage

| Building | Capacity | Notes |
|---|---|---|
| Storage Depot v.1 (resource-redistributor) | 400 (one item) | Clever L3 |
| Storage Depot v.2 (storage) | 1,600 (one item) | 5 power |
| Expandable Storage (storage-depot) | 1,600+, expands with more power | 60 power, 200 heat — expensive |
| Multistorage (universal-storage) | 2,500, **multiple item types** | Clever L12. "Stores excess items that can not be sent anywhere else" — acts as an overflow sink |
| Delivery Storage | — | "Send or request items through the drone system" (site-flagged hidden; verify in game) |
| Personal Storage / Large | — | Habitat-interior player storage |

## Long-distance & special transport
- **Cargo Dispatcher + Cargo Receiver** (package-sender/receiver, Clever L7, 40 power each) — long-distance point-to-point item transport without running rail the whole way.
- **Teleporter** (Clever L9, 80 power) — player travel between any two teleporters.
- **Zipline** (Clever L6) — cheap player traversal; does NOT conduct electricity.
- **Defense Tower (turret t2)** can be resupplied with ammo **by drone rail** — plan an ammo spur into defense positions (Defense Turret t1 is manual-feed only).

## Throughput math that matters (Update 1 numbers)

Extraction outputs vs rail capacity (see [[Star Rupture production]] for the full rate table):
- Ore Excavator v.1 on a **Normal** node: 120/min → exactly saturates one Rail v.1.
- Ore Excavator v.1 on a **Pure** node: 240/min → needs Rail v.2 (or two v.1 lines).
- Ore Excavator v.2 on Normal: 300/min → Rail v.3 territory (480) or split across two v.2 rails.
- Ore Excavator v.2 on Pure: 480/min → exactly saturates one Rail v.3.
- Helium-3 Extractor / Sulfur Extractor: 240/min each → one Rail v.2 per extractor, or pair two onto a Rail v.3 (480).

Rule of thumb: **rail tiers double (120/240/480), and extractor rates land exactly on those breakpoints** — the game is designed for 1:1 saturated lines if you match tier to source. Merging with Rail Modulators only helps when downstream rail tier > sum of upstream flows.

## Layout thinking (first-pass guidance, refine with play)
- **Backbone + spurs**: because rails conduct power, a v.2/v.3 trunk from the Base Core area to each mining outpost carries both power out and ore back. Junctions/Roundabouts at district boundaries.
- **Match rail tier to line rate**: don't upgrade everything — a saturated Rail v.1 (120/min) feeding one Furnace bank is fine; upgrade only trunks that aggregate multiple sources.
- **One item per line where possible**: single-item storage depots (400/1,600) act as buffers per line; Multistorage catches overflow at the end of a bus so lines never back up into producers.
- **Exporter placement**: Orbital Cargo Launcher takes 1 input — dedicate one per export product, fed from a buffer depot, so reputation flow never starves.
- **Base Core capacity is the real limit** on how much logistics+production you can pack into one base (see heat budget in [[Star Rupture production]]) — distributed mining outposts connected by trunk rails beat one mega-base once capacity gets tight.
- Turret ammo spur: run a thin Rail v.1 loop to Defense Towers from the ammo printer.

Open questions to verify in-game: how drones queue at Rail Modulator merges (round-robin vs priority), whether Multirail lanes share throughput or are independent rails, Drone Station coverage radius, and Delivery Storage availability in Update 1.

Created: 2026-07-07
Updated: 2026-07-07

# Star Rupture production

Machines, extraction rates, recipes, power and Base Core capacity ("heat") budgets. Data: starrupture.tools crawl 2026-07-07; game version EA Update 1 (0.2.x). Full recipe data in `data/starrupture.sqlite` (`recipes`, `recipe_inputs` — includes computed items/min).

## Extraction rates (items/min) — Update 1

Ore nodes come in **Impure / Normal / Pure** grades (map marker counts on Arcadia-7: Titanium 58/86/36, Wolfram 15/39/31, Calcium 19/38/19).

| Extractor | Impure | Normal | Pure | Power | Heat |
|---|---|---|---|---|---|
| Ore Excavator v.1 (Ti/W/Ca) | 60 | 120 | 240 | 5 | 3 |
| Ore Excavator v.2 | 180 | 300 | 480 | 30 | 18 |
| Laser Drill (Ti/W/Ca) | 15 | 30 | 120 | 250 | 200 |
| Laser Drill (Goethite — its real job) | — | 15 | — | 250 | 200 |
| Helium-3 Extractor | — | 240 | — | 15 | 60 |
| Sulfur Extractor | — | 240 | — | 40 | 60 |
| Oil Extractor (Powerium/"magic oil") | — | 10 | — | 400 | 300 |

Notes: Laser Drill is required for **Goethite** (advanced ore, unlock level 8-ish); on basic ores it's worse than the mechanical drills — don't use it there. Ore Excavator v.2 on Pure = 480/min = exactly one Rail v.3 (see [[Star Rupture logistics]]).

## Production machines

| Machine | Tier path | Power | Heat | Sockets in/out | Recipe levels | Role |
|---|---|---|---|---|---|---|
| Smelter | — | 5 | 3 | 1/1 | 0 | Raw Ti/W/Ca ore → blocks |
| Basic Item Printer (item-printer) | — | 0 | 0 | interior | 0–8 | Building materials + ammo, inside Habitat |
| Fabricator (Crafter) | → v.2 (25 pw, 8 heat, 3 in) | 10 | 5 | 2/1 | 0–5 | Basic parts |
| Furnace | → v.2 (50 pw, 14 heat, 4 in) | 20 | 8 | 3/1 | 2–6 | Heat processing (powders, ceramics) |
| Mega Press (Hammer) | — | 30 | 15 | 4/1 | 0–12 | High-speed pressing |
| Compounder (Synthetizer) | → v.2 (180 pw, 250 heat) | 60 | 150 | 3/1 | 6–10 | Chemistry |
| Assembler | — | 60 | 150 | 3/1 | 7–11 | Intermediate assembly |
| Pyro Forge | — | 200 | 180 | 2/1 | 8 | Goethite refining |
| Refinery (Pressurizer) | — | 150 | 100 | 3/1 | 9 | Liquids/pressure |
| Constructorizer (Factory) | → v.2 | 700 | 500 | 4/1 | 8–12 | Advanced products |
| Facturer (Military Assembler) | — | 850 | 650 | 5/1 | 10–13 | Top-end/military |

"Recipe levels" = the progression level stamped on each recipe (gates when you can research it; correlates with machine unlock order). Level 10–15 recipes include Forgotten Engine endgame items (arc reactor, neutrino missile, satellite assembly) — treat as endgame/possibly future content.

## Power generation

| Generator | Power | Heat | Unlock |
|---|---|---|---|
| Solar Generator v.1 | +10 | 5 | Moon L1 / starting |
| Solar Generator v.2 | +140 | 20 | Development Station |
| Interior Power Generator | +100 | 0 | inside habitat; immune to star waves |
| Pressure Power Generator | +1,000 | 0 | geysers only |
| Wind Turbine v.1 | +1,200 | 100 | Moon L9 |
| Wind Turbine v.2 | +3,200 | 200 | Development Station |
| Chemical generator (combustion) | +8,200 | 400 | Moon L13; burns atmospheric chemicals |
| Chimeran Power Generator | +2,000 | 0 | site-flagged hidden — likely NOT in Update 1 |

Note solar is nearly useless past the opening (10 power vs a Furnace's 20 draw). Wind Turbine v.1 at Moon L9 is the first serious generator; note generators themselves add heat.

## Base Core capacity (the heat budget)

Base Core: "Allows building other constructions. Protects your base from Fire Wave. **Exceeding capacity disables Fire Wave protection**."

| Base Core level | Capacity | Upgrade cost (cumulative per level) |
|---|---|---|
| 0 | 1,000 | — |
| 1 | 2,500 | 30 BBM, 7,200 Helium-3, 1,800 Ceramics, 10 Ignitium |
| 2 | 4,000 | 50 BBM, 2,700 Ceramics, 600 Synthetic Silicon, 600 Electronics, 30 Ignitium |
| 3 | 6,000 | 60 BBM, 400 Valve, 800 Electronics, 900 Battery, 90 Ignitium |
| 4 | 10,000 | 70 BBM, 800 Turbine, 1,200 Electronics, 2,300 Hardening Agent, 800 Accumulator, 150 Ignitium |

(BBM = Basic Building Material. Costs from base-core levels data; verify item mix in-game.)

Capacity add-ons: **Base Core Amplifier v.1** (cooler-active) "+50 capacity" and **Amplifier v.2** (cooler-passive) "+20 capacity" per site descriptions — but their data `temperature` fields read −250 and −20 respectively. Discrepancy flagged; verify actual numbers in-game.

Budget example: a Constructorizer (500) + Facturer (650) together eat more than a level-0 core's entire 1,000 capacity. Heavy machines force core upgrades or satellite bases.

## Chain examples (per-min rates from the DB)

Smelting: each ore → block via Smelter. Ore Excavator v.1 on Normal (120 ore/min) needs matching smelter throughput — check `SELECT * FROM recipes WHERE buildings='smelter'` for cycle rates and size smelter banks accordingly.

Key early intermediates (query `recipe_inputs` for exact ratios):
- **Basic Building Material** — Material Crafter recipe: 1 Wolfram Ore + 1 Titanium Ore → 10 BBM / 5s (120/min). Also craftable from the Basic Item Printer.
- **Basic Electronics** — 1 Wolfram Bar + 2 Cables → 1 / 3s (20/min).
- **Ceramics (v2 recipe)** — Calcite Sheets + 2 Helium-3 (+ …) → level 3 Furnace v.2 recipe, 2s cycle.
- **Calcium Powder v2** — 2 Calcium Block → 10 / 3s (200/min) in Furnace v.2.

Useful queries:
```sql
-- what consumes item X
SELECT r.id, r.buildings, ri.quantity FROM recipe_inputs ri JOIN recipes r ON r.id=ri.recipe_id WHERE ri.item_id='helium-ore';
-- full ratio sheet for a machine
SELECT r.id, r.duration, r.output_item, r.output_qty, r.per_min FROM recipes r WHERE buildings LIKE '%furnace%' ORDER BY r.level;
```

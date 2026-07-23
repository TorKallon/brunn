Created: 2026-07-07
Updated: 2026-07-07

# Star Rupture

First-person open-world factory/base-building + survival game by **Creepy Jar** (makers of Green Hell). Solo or co-op up to 4. You play a prisoner sent to the planet **Arcadia-7**: mine resources, build an automated factory, survive periodic **Fire Waves** (star ruptures), fight alien creatures ("Vermin"/Chimeran), and export products to orbital **Corporations** for reputation and unlocks.

## Version context (important)
- **Steam Early Access release: January 6, 2026** (playtests: solo July 2025, co-op Dec 3–8 2025 — info from those eras is outdated).
- **Currently playing: Early Access Update 1**, patch line 0.2.x (0.2.8 hotfix ~June 17, 2026).
- Update 2 is in development (dev blog June 19, 2026 lists WIP features like unified items/min crafting UI).
- See [[Star Rupture Update 1 research]] for verified patch-note/community findings.

## Source quality guide
- **starrupture.tools** — best data source; current and maintained. CAVEATS: it includes future-facing/not-yet-obtainable content mixed in with live content. Known future/unreleased: **Rail v.4 (750/min) and Rail v.5 (1500/min)** (confirmed not in game in Update 1), 27 `hidden`-flagged buildings (Chemical Processor, Water Extractor, Deuterium Extractor, Chimeran Power Generator, etc.), and level 10–15 recipes tied to the endgame/Forgotten Engine scenario.
- **Steam news/patch notes from Creepy Jar** — authoritative for what's actually in the build.
- **Avoid**: guides written against the July/Dec 2025 playtests, and generic AI-generated articles (fabricated stats are common — e.g., invented ore names or belt speeds not matching the tables in [[Star Rupture logistics]]).

## The five core systems
1. **Power** — buildings consume power (negative values) or generate it (Solar 10 → Chemical generator 8,200). Rails conduct electricity, so the logistics network is also the power grid. See [[Star Rupture production]].
2. **Base Core capacity ("heat")** — every powered building adds load (its `temperature` stat) against the Base Core's capacity (1,000 at level 0 → 10,000 at level 4). **Exceeding capacity disables Fire Wave protection** — the base gets wrecked during the next wave. Amplifiers (coolers) add capacity. This is the primary constraint on base density. See [[Star Rupture production]].
3. **Logistics (drone rails)** — items move on rail segments between building sockets; throughput per rail tier: v1 120/min, v2 240/min, v3 480/min. See [[Star Rupture logistics]].
4. **Corporations & exports** — 5 corporations, each a themed unlock track (levels via reputation from exporting goods). See [[Star Rupture progression]].
5. **Survival & defense** — Fire Waves on a timer (shelter inside Base Core radius / habitats), creature attacks on the base (turrets; Defense Tower t2 can be ammo-fed by rail), food/water/toxicity meters.

## Notes in this topic
- [[Star Rupture logistics]] — rails, junctions, mergers, storage, exporters, factory layout thinking
- [[Star Rupture production]] — machines, extraction rates, recipes, power and heat budgets
- [[Star Rupture progression]] — corporations, research/data points, blueprints, base core levels
- [[Star Rupture Update 1 research]] — verified findings about the current patch
- `Topics/Star Rupture/data/` — SQLite DB + CSVs of the full game database (see data/README)

## Quick data access
All stats live in `Topics/Star Rupture/data/starrupture.sqlite` (+ CSV mirrors). Example:
```sql
-- items/min a machine produces for each recipe
SELECT id, buildings, duration, output_qty, per_min FROM recipes WHERE buildings LIKE '%assembler%';
```
Tables: items (468), buildings (218), recipes (234), recipe_inputs, building_costs, research_costs, exports, corporation_levels/rewards, base_core_levels, analysis, map_markers (3,259), weapons, xp_actions.

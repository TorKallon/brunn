Created: 2026-07-10
Updated: 2026-07-11

Aliases: Star Rupture, SR

## Purpose

Stable home for StarRupture gameplay knowledge, Update history, factory planning, progression questions, map and loot research, and reproducible scenarios.

The structured corpus is designed to answer cross-system questions rather than behave like a pile of copied wiki pages. Items, recipes, buildings, unlocks, exports, upgrades, map markers, loot, weapons, patch changes, and curated mechanic facts share version and provenance keys in one SQLite database.

## Start here

- [[Topics/StarRupture/Early Access - Update 1|Early Access - Update 1]] — extensive launch-to-0.2.8 research, exact design changes, reward redistribution, PTB caveats, and hotfix lineage.
- [[Topics/StarRupture/Game mechanics - Update 1|Game mechanics - Update 1]] — system map and reasoning boundaries.
- [[Topics/StarRupture/Factory logistics planning - Update 1|Factory logistics planning - Update 1]] — rail breakpoints, layout heuristics, buffering, export, and open logistics questions.
- [[Topics/StarRupture/Production planning - Update 1|Production planning - Update 1]] — quick-reference extraction, machine, power, and Base Core planning guidance.
- [[Topics/StarRupture/Player save - sites and freight network|Player save - sites and freight network]] — canonical names, installed bases, satellites, freight contracts, and the second Supermagnet decision.
- [[Topics/StarRupture/Player save - play history|Player save - play history]] — chronological factory evolution, incidents, exploration pivot, direct observations, and resume questions.
- [[Topics/StarRupture/Player save - operating model and factory architecture|Player save - operating model and factory architecture]] — bursty demand, hybrid topology, buffer policy, and superseded designs.
- [[Topics/StarRupture/World geography and named locations|World geography and named locations]] — imported site screenshots, annotated map, and landmark-relative directions.
- [[Topics/StarRupture/Player save - progression and next builds|Player save - progression and next builds]] — confirmed state, unknowns, Flowworks stages, GB-A, and current continuation route.
- [[Topics/StarRupture/Player save - exploration and combat|Player save - exploration and combat]] — Faraday launch loops, Engine preparation, and unresolved inventory questions.
- [[Topics/StarRupture/Food strategy - permanent low-chore system|Food strategy - permanent low-chore system]] — Nutri Block operating plan and medium-confidence effect assumptions.
- [[Topics/StarRupture/Remote resource satellites - Goethite and Oil|Remote resource satellites - Goethite and Oil]] — permanent Cargo topology for the safer Goethite triple and first Oil source.
- [[Topics/StarRupture/Knowledge base and modeling|Knowledge base and modeling]] — data versions, SQLite schema, refresh workflow, queries, and production models.
- [[Projects/RuptureOps/RuptureOps|RuptureOps]] — iOS companion product planning and project status.
- [[Projects/RuptureOps/Player-derived product requirements|Player-derived product requirements]] — real-play evidence translated into app requirements and a candidate first native slice.
- [Current SQLite knowledge base](</Users/Shared/projects/ruptureops/data/star-rupture.sqlite3>)
- [Current snapshot pointer](</Users/Shared/projects/ruptureops/data/current.json>)

## Current version boundary

The corpus keeps two separate clocks:

| Axis | Current value | Meaning |
|---|---|---|
| SRDB game-data claim | Update 1, internal game version `35118973`, dated 2026-04-09 | The build context claimed by `starrupture.tools` for its numerical data. It maps contextually to Update 1 Steam build `22674441`; SRDB does not publish that equivalence directly. |
| Source capture | SRDB `2.3.4`, site updated 2026-07-10, captured 2026-07-11 05:02:30 UTC | Immutable source snapshot. Later SRDB corrections get a new capture rather than overwriting this one. |
| Latest public Update 1 game build at capture | Hotfix `0.2.8`, Steam build `23761620`, released 2026-06-17 | Current public executable lineage, established from official patch history. It is not silently assigned to every SRDB number. |

That distinction is intentional. “What is true in the current public game?” and “what exact values did the captured database publish?” are related but not identical questions.

## Corpus coverage

The initial versioned snapshot contains:

| Dataset | Rows |
|---|---:|
| Items | 468 |
| Buildings | 218 total: 191 public listing, 27 hidden/internal |
| Recipes | 234 |
| Researchable recipes | 137 |
| Corporations / levels | 6 / 77 |
| Export rows / non-zero offers | 102 / 121 |
| Development Station upgrades / tasks | 15 / 32 |
| Item analyses | 54 |
| XP sources | 25 |
| Map markers | 3,259 |
| Loot tables / entries | 1,384 / 3,094 |
| NPC spawn markers | 1,063 |
| Radiation areas | 2 |
| Weapons / modification cards | 4 / 35 |
| SRDB guides | 9 |
| Official Steam announcements retained | 56 |
| Asset references / available logical assets | 1,081 / 466 |

All 745 indexed or explicitly seeded SRDB RSC routes were captured successfully. Raw payloads, normalized JSON, response hashes, source metadata, and 454 unique asset blobs are retained in the immutable snapshot directory. Twenty-five source asset paths return genuine 404s and remain explicitly indexed.

## Ready-to-run questions

The local tooling can already answer or model:

- What makes an item, what consumes it, and where does it unlock?
- What is the recursive bill of materials for a target items-per-minute rate?
- How many machines are theoretically required, and what are the independent rounded machine counts?
- What raw extraction rates feed the chain?
- What are the provisional power and Base Core capacity totals from SRDB raw values?
- Which corporation and level rewards an item or building?
- What can be exported, to whom, at what level and value?
- What yields Data Points or XP?
- Where can an item appear, and which loot tables reference it?
- Which enemies or AI archetypes occur around a map coordinate?
- What changed between future source snapshots?

The raw source has 48 recipes without a building association. Six imported,
medium-confidence Ore Excavator v2 purity associations leave 42 unresolved in
the active database. Plans that encounter one return `status: incomplete` and
an unresolved-production ledger; they do not misclassify the target as a raw
material.

The model deliberately does not yet pretend to solve unverified systems such as drone path priority, Fire Wave downtime, loot expected value, exact structural stability, or full unlock-path optimization.

## Fast examples

From [RuptureOps scripts](</Users/Shared/projects/ruptureops/scripts>):

```bash
/opt/homebrew/bin/python3 query.py stats
/opt/homebrew/bin/python3 query.py search 'Cargo Dispatcher'
/opt/homebrew/bin/python3 query.py item organic-compound
/opt/homebrew/bin/python3 query.py recipe superconductor
/opt/homebrew/bin/python3 query.py plan organic-compound --rate 45
/opt/homebrew/bin/python3 query.py near 3000 2200 --radius 600 --type '%Ore%'
/opt/homebrew/bin/python3 validate.py
```

The 45 Organic Compound/minute scenario reproduces the live SRDB crafting API's seven raw-material rates exactly.

## Working conventions

- Pin numerical answers to a source snapshot.
- Pin build-specific mechanics to a game build whenever the evidence supports it.
- Treat Steam build IDs as immutable release keys; `0.1.0` and `0.2.0` are high-confidence inferred labels because the corresponding launch posts did not print them.
- Keep `starrupture.tools` raw IDs and spelling intact; cleaned names live in an alias layer.
- Preserve hidden and prototype content, but exclude it from player-facing model defaults.
- Keep intended design changes, historical bugs, fixes, migrations, inferred facts, and community-tool logic as different fact kinds.
- Do not calculate expected loot value until probability semantics are verified.
- Do not mix coordinate systems without a verified transform.
- Do not publish the mirrored dataset as a public derivative without permission; SRDB permits crawling but publishes no explicit data license.
- Treat player-reported installed state, adopted plans, and generated analysis as different evidence classes. In particular, Flowworks and Grand Basin are selected/planned, not confirmed built.
- Treat direct prompt history, quoted assistant material, questions, hypotheses, and verified corpus facts as separate evidence classes. A historical checkpoint does not silently become current state.
- The imported July 8 analysis package is audit evidence, not a second source snapshot. Its `23761620` label identifies the latest public build observed, not the provenance of every numerical value.
- The July 11 player-prompt import preserves all three user-authored dumps losslessly. Its overlapping statements are deduplicated into one play-history model rather than counted as separate facts.

## Related

- [[Topics/Gaming/Gaming|Gaming]]
- [[Projects/RuptureOps/RuptureOps|RuptureOps]]
- [[Home]]
- [[INDEX|Shared knowledge index]]
- [[Vault rules]]

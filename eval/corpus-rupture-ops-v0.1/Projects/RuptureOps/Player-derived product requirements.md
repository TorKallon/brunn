Created: 2026-07-11
Updated: 2026-07-11

Related: [[Projects/RuptureOps/RuptureOps|RuptureOps]], [[Topics/StarRupture/Player save - play history|Player save - play history]], [[Topics/StarRupture/Player save - sites and freight network|Sites and freight network]], [[Topics/StarRupture/Player save - operating model and factory architecture|Operating model and architecture]]

# Player-derived product requirements

## Product thesis

RuptureOps should be a companion for an actual, changing save—not a smaller static wiki and not only a steady-state calculator. Its job is to help a player decide what to build or do next, using the machines, locations, buffers, unlocks, time budget, and play style they really have.

The defining loop is:

```text
build and refill → leave for exploration/combat → return to accumulated stock
→ start a finite campaign → diagnose what fails to recover → expand deliberately
```

The app should optimize time to the next meaningful goal, recoverability, and clarity. Maximum theoretical throughput is supporting evidence, not the whole product.

## Evidence-backed requirements

| Real play evidence | Product requirement |
|---|---|
| The save has active, selected, planned, historical, uncertain, and superseded states. | Every player fact needs a state and an `as of` checkpoint. Plans must never silently become installed machines. |
| Faraday's universal bus was delightful to extend but impossible to diagnose after shortcuts and bypasses. | Preserve a low-friction “add one machine” workflow while showing requester paths, cycles, shared-lane refill spikes, and likely bottlenecks. |
| Turn-ins are finite, new products often start at zero, inputs are usually buffered, and the player spends hours away. | Campaign timing must combine target quantity, current inventory, machine counts, input buffers, refill rates, and expected away time. Storage may accelerate a batch but cannot masquerade as sustainable capacity. |
| V2 buildings are usually better, but sometimes require a resource a mile away. | Let players mark owned building tiers and compare V1/V2 recipe paths. Explain distance, onsite inputs, heat, freight, footprint, and batch-size tradeoffs rather than forcing one “optimal” recipe. |
| The planet is organized as named specialist sites and campuses with storage-yard boundaries. | Give every chain a site/campus owner, installed and planned machine counts, Core/heat budget, imports, exports, storage, and reserved expansion space. Show named geography before raw coordinates. |
| Pure Titanium and merged Helium/Sulfur sources were limited by their rails. | Compare node/extractor rate, aggregate producer rate, rail tier, refill-wave demand, and destination recovery. Separate local buses from the world trunk. |
| Copperfield ended in death after ammunition ran out; the player wants low-twitch Engine plans. | Expedition plans need threat level, owned loadout, ammo/healing budget, abort points, empty-slot target, ordered rewards, and return depot. Calibrate to player skill instead of assuming ideal gunplay. |
| The first Geo Scanner took two deaths: one nearby respawn base had no replacement kit, and corruption later disabled the contested fire base's Regeneration Chamber; success came only after every turret was powered. | Scanner plans need a pre-activation readiness gate covering turret power and firing checks, a separately safe recovery cache, replacement weapons/ammo/healing/build materials, corruption-aware respawn redundancy, and an explicit worst-case return point. |
| Food had too many choices; actual forage was Hydrobulb > Polufruit > Prickler; a one-slot balanced item was desirable. | Food planning should use owned recipes, actual gathering mix, trip duration, toxicity, slot budget, and research costs. Clearly distinguish unlock consumption from finished output. |
| Ammo/building-material Dispatchers were considered for temporary firebases. | Support reusable expedition supply kits and a future Dispatcher/Receiver sufficiency planner for Geo Scanner or raid staging. Keep this out of installed state until built. |
| The player repeatedly asked to see saved visualizations and exact per-site tables. | Plans must be saveable, reopenable, comparable, and shareable. Use plain English, exact math, visible assumptions, and base-scoped outputs. |
| Questions shifted among factory, food, map, combat, and progression, with an explicit request not to lose earlier asks. | Maintain a multi-goal session queue and show which goals one outing or build advances. |

## Core player profile

- Prefers active play and strategic preparation over AFK waiting.
- Likes building several modest specialist factories; dislikes repeating a large dependency chain.
- Values buffers, but rejects buffer multiplication as a substitute for machines.
- Prefers debuggable trees and visible storage interfaces over opaque meshes.
- Wants recommendations that can disprove an idea, not merely validate it.
- Wants conservative combat preparation and landmark-grounded directions.
- Wants concise, plain-language answers backed by exact math.

## Candidate first native slice

The smallest slice that exercises the distinctive product thesis is a **save-aware campaign planner**:

1. Select a target Corporation/research/export batch.
2. Choose the producing site and owned building tiers.
3. Enter installed machine counts and current input/output buffer fill.
4. Compare recipe alternatives and predicted completion/recovery time.
5. Show the smallest useful build change, its site-local inputs, rail load, power/Core effect, and assumptions.
6. Save the scenario against the current source snapshot and player checkpoint.

This is a candidate, not a locked MVP decision. A static encyclopedia or generic recipe tree would be easier, but it would not test the player-specific value revealed by the prompt history.

## Required evidence model

Use at least these states:

- `confirmed_current`
- `confirmed_historical`
- `player_observation`
- `player_hypothesis`
- `selected_not_built`
- `planned_not_built`
- `assistant_recommendation`
- `verified_corpus_fact`
- `unknown`
- `superseded`

Every numerical scenario must retain the structured-data snapshot. Player checkpoints need their own date or sequence marker; they must not borrow the game's build version as if it were a save timestamp.

## Native dogfood constraint

This is intended to become a real native iOS dogfood app used on the player's personal phone. The development Mac cannot use OS-level iCloud login, so signing, account, and device deployment need a flow that does not assume that login path. The exact employment context stays in the private raw prompt history and is not a product requirement.

## Non-goals

- Do not reproduce the entire source site as a static wiki.
- Do not assume every player wants one giant balanced factory.
- Do not call the geometrically nearest resource the best site without route, threat, defense, and failure-domain context.
- Do not assume recommended machines, weapons, mods, or blueprints are owned.
- Do not hide uncertainty or convert a question into a fact.
- Do not make giant buffers the default answer to inadequate production.

## Questions for product continuation

1. Is the first hands-on journey campaign timing, a saved-site inventory, or an expedition planner?
2. How much player state should be entered manually in v1, and what can later be imported from a save?
3. Should the first plan view be a compact table, a dependency graph, or both?
4. Which offline dataset and source-provenance fields must ship in the first test build?
5. What is the minimal signing/deployment setup for the personal-phone dogfood loop on this Mac?

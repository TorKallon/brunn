Created: 2026-07-11
Updated: 2026-07-11

Related: [[Topics/StarRupture/Player save - exploration and combat|Exploration and combat]], [[Topics/StarRupture/Player save - play history|Play history]], [[Topics/StarRupture/Player save - operating model and factory architecture|Operating model]], [[Topics/StarRupture/StarRupture|StarRupture]]

# Food strategy - permanent low-chore system

## Evidence boundary

- StarRupture Update 1 through Hotfix 0.2.8.
- Assumes every Food Station recipe is unlocked.
- Recipe quantities use current Update 1 data rather than initial Early Access values.
- Nutri Block's visible countdown reportedly blinks red during its final minute; no prominent audio warning has been verified.
- Consumable effect values remain medium-confidence community/item-page findings until verified in game or by a first-party source. Canonical assumptions are retained in [consumable-effects.json](</Users/Shared/projects/ruptureops/sources/consumable-effects.json>).

## Player constraints and supply profile

Play alternates between long factory sessions and hours of exploration/combat. The food system should minimize attention, inventory opening, repeated clicks, and stack count while maintaining large buffers.

Figler was tested and rejected as tedious: it yields too little food for the gathering effort, requires too many uses, and consumes Prickler needed by the prevention system. The direct prompt history records the moment as “unlocked it then didn't have anything left.” Separately, the structured model lists 30 Prickler plus 100 Data Points as research inputs and the finished Figler as a later Food Station craft using 3 Prickler.

Common forage frequency is Hydrobulb, then Polufruit, then Prickler, with much smaller quantities of rare mixers. The direct prompt history also records substantial continued spawning at a lake crossed by a rail and zipline but no Platforms. That rejects a blanket “any nearby structure stops respawns” rule without establishing whether Platforms suppress them.

Recorded inventory at the strategy decision:

| Item | Quantity |
|---|---:|
| Prism Herb | 318 |
| Glowcap | 157 |
| Hydrobulb | 97 |
| Polufruit | 82 |
| finished Hydrolite | 64 |
| Prickler | 42 |
| Oxallop | 27 |
| Star Tears | 24 |
| Grubbler | 9 |
| Serpent Root | 4 |
| finished Nutri Gel | 2 |

All 157 Glowcaps represent the player's lifetime cave harvest and have not been consumed. They are a strategic combat-medicine reserve, not routine food.

## Three-item operating system

### 1. Nutri Block = prevention

- Recipe: 5 Prickler + 1 Oxallop -> 1.
- Stops natural calorie and hydration loss for 30 minutes.
- Adds 45 toxicity.
- Use from full meters and low toxicity.
- Expiration only resumes normal drain; it does not suddenly empty the meters.

### 2. Calorie Chew = calorie recovery

- Recipe: 5 Polufruit + 1 Prism Herb -> 3.
- Each chew restores 40 calories.
- One batch restores 120 calories total.

### 3. Aqua Chew = hydration recovery

- Recipe: 4 Hydrobulb + 1 Prism Herb -> 1.
- One chew restores 100 hydration.

The standard complete refill is **3 Calorie Chews + 1 Aqua Chew**, restoring 120 calories and 100 hydration with four uses across two stacks.

## Operating routine

1. Fill both meters with 3 Calorie Chews + 1 Aqua Chew.
2. Start one Nutri Block.
3. Carry two Blocks per planned hour plus two extra overall.
4. Carry one backup refill: 3 Calorie Chews + 1 Aqua Chew.
5. Double the Chew backup for a very long trip.
6. If the Block expires while meters remain high, take the next Block when toxicity permits.
7. If the meters have fallen substantially, refill with Chews before resuming Blocks.
8. Add Glowcaps only when combat healing is warranted.

Do not use Purfins, Antidote Gel, or another item that removes all positive effects while a Block is active. Clear Capsules are the later compatible detox option because they accelerate natural toxicity loss without stripping the desired buff.

## Existing-stock conversion

- 80 Polufruit + 16 Prism -> 48 Calorie Chews, leaving 2 Polufruit.
- 96 Hydrobulb + 24 Prism -> 24 Aqua Chews, leaving 1 Hydrobulb.
- Total Prism remaining: 278.
- 40 Prickler + 8 Oxallop -> 8 Nutri Blocks/four hours, leaving 2 Prickler and 19 Oxallop.

Consume the 64 existing Hydrolites rather than discarding them. Two Hydrolites can replace one Aqua Chew until depleted. Consume the two Nutri Gels and miscellaneous finished food opportunistically, but do not replenish them.

## Glowcap policy

- Preserve the strategic reserve; treat the 50 HP healing as the primary effect and food restoration as a bonus.
- Safe local work: carry none.
- Ordinary combat exploration: carry 5-10.
- Forgotten Engine: carry 25-40.
- Pick every cave Glowcap encountered and bank it.

## Do not routinely manufacture

- Figler or Polisnack.
- Additional Hydrolite.
- Nutri Chew, Gel, Bar, or Fluid.
- Aqua Bar, Gel, or Fluid.
- Calorie or Aqua Stim Shards.

Reasons:

- Figler consumes the Prickler bottleneck and is slightly worse than eating its inputs raw.
- Nutri Chew has good direct-food efficiency but competes with Nutri Blocks for Prickler.
- Aqua Bar spends scarce Serpent Root needed for energy consumables.
- Aqua Gel spends scarce Grubblers.
- Aqua Fluid has worse hydration efficiency than raw Hydrobulb and spends Star Tears.
- Stim Shards increase capacity but neither refill meters nor prevent drain.

## Alternative Block system

Calorie Block + Aqua Block costs 4 Polufruit + 4 Hydrobulb + 2 Oxallop per hour. Nutri Blocks cost 10 Prickler + 2 Oxallop per hour. Both require two uses and add 90 total toxicity per hour.

Nutri Block remains the default because it occupies one stack and spreads toxicity across two 45-point doses. Reconsider the separate pair only if Prickler gathering becomes intolerable; the pair occupies two stacks and applies 90 toxicity simultaneously.

## Buffers and gathering priorities

| Supply | Minimum | Comfortable |
|---|---:|---:|
| Nutri Blocks | 20 | 40 |
| Calorie Chews | 30 | 60 |
| Aqua Chews | 10 after Hydrolites are depleted | maintain as needed |
| Glowcaps | preserve reserve | preserve reserve |

Gather in this order:

1. Prickler is the immediate bottleneck.
2. Polufruit is the next ordinary-food priority.
3. Collect Oxallop aggressively during convenient post-Rupture dry-lake windows.
4. Take every Glowcap encountered in caves and save it for healing.
5. Hydrobulb and Prism are overstocked; take only path-convenient pickups for now.
6. Pick scarce Serpent Root and Grubbler whenever encountered, but do not spend them on routine nutrition.

After producing the first eight Nutri Blocks, gather **58 additional Prickler**. Combine it with the two remaining Prickler and 12 Oxallop to make 12 more Blocks. The result is the 20-Block minimum buffer, covering ten hours; no additional Oxallop is required for this first target.

## Ultra-compact reference

**Polufruit -> Calorie Chew. Hydrobulb -> Aqua Chew. Prickler -> Nutri Block. Glowcap -> medicine.**

- Full refill: 3 Calorie Chews + 1 Aqua Chew; use 2 old Hydrolites instead of the Aqua Chew while they last.
- Depart: fill meters, take 1 Block, carry 2 Blocks/hour + 2 spares, and carry one full Chew refill.
- Combat: add 5-10 Glowcaps; Forgotten Engine: 25-40.
- Gather next: 58 Prickler -> 12 Blocks -> 20 Blocks/10 hours total.
- Rebuild at: Blocks 20/40, Calorie Chews 30/60, Aqua Chews 10 after Hydrolites.

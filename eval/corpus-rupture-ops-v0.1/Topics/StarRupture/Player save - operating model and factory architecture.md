Created: 2026-07-11
Updated: 2026-07-11

Related: [[Topics/StarRupture/Player save - sites and freight network|Sites and freight network]], [[Topics/StarRupture/Player save - play history|Play history]], [[Topics/StarRupture/Factory logistics planning - Update 1|Factory logistics]], [[Topics/StarRupture/Production planning - Update 1|Production planning]], [[Projects/RuptureOps/Player-derived product requirements|Player-derived product requirements]], [[Topics/StarRupture/StarRupture|StarRupture]]

# Player save - operating model and factory architecture

## The planning target

Rourke's factory is not a permanently balanced Satisfactory-style plant. The save has bursty, pull-driven demand:

- exploration and combat outings often last hours;
- Corporation/OCL requests are finite and often unrelated;
- full destination buffers correctly stop upstream traffic;
- idle machines are often healthy;
- accumulated stock during active play is valuable;
- AFK-only completion caused by chronic undersizing is not desirable.

Two hours is an acceptable ceiling for an awkward late campaign, not the target wait for every unlock. Large buffers are useful interfaces, but “add a second depot instead of a productive machine” was explicitly rejected as lazy and slow.

The later, sharper service preference is campaign-specific:

- below roughly level 12, target under one hour and allow up to two hours only rarely;
- for final campaigns, prefer one to two hours while accepting an occasional three to four hours;
- count accumulated stock honestly, but do not use massive storage or AFK time to hide chronic undersizing.

A few new modest specialist factories are enjoyable. Rebuilding the same large dependency chain repeatedly is tedious. Cheap extraction can therefore be deliberately overbuilt for cold-start recovery when adding drills and Dispatchers is trivial but duplicating a hot processing chain is not.

## Default architecture

Use four layers:

1. **Evergreen backbone:** continually replenish broadly reused materials, ammunition, building supplies, and consumables.
2. **Slow-fill reserves:** one explicit interface buffer for important advanced intermediates.
3. **Campaign cells:** a small number of machines sized for the current finite batch.
4. **Player-facing turn-in hub:** collect finished campaign output near the Habitat/OCL, outside production districts.

Default recipe choice is v2-first when it improves throughput, footprint, power per item, or raw-material efficiency. Always preserve a local/v1 fallback and name its switch condition: unavailable building/input, excessive heat, distance, freight, or poor batch fit.

## What the first factories taught us

### Landerworks: dedicated spaghetti that still works

The first base evolved from dedicated forward and return rails into shared returns with dedicated forward routes. It became difficult to read, but remained reliable at its limited Rotor/Stator/Applicator role. The durable lesson is not “old layouts must be rebuilt”; it is “stable specialists can remain leaves.”

### Original Faraday: a delightful bus that became opaque

The original second base put Ore and ingots on dedicated lines, then shared Plate/Wire/Powder and higher products on a convenient global loop. Its best property was genuinely delightful: connect one input, select the machine's output, and return it to the bus. That low-friction extension model made the player unusually productive.

The loop eventually became long, acquired shortcuts and high-volume bypasses, and produced multiple possible requester paths. Pulls appeared to deadlock, but the root cause is unproven: routing bug, request starvation, hidden contention, or a connection error could all fit. After roughly two hours of failed diagnosis and retrofit, the player tore down every machine in Faraday.

The design failure was not sharing. It was allowing one convenient bus to become the entire dependency graph.

### Rebuild experiment: useful ideas inside a superseded layout

The first rebuild sketch used elemental shape buses plus a low-volume U-shaped central bus. Cross-element recipes still forced long converging runs, so that exact geometry was not the final answer. Two patterns survived: the Ceramics → storage → Synthetic Silicon inner loop, with slack exposed to secondary users, and the direct Chemicals → Acid dominant-consumer feed.

### Rebuilt Faraday: hybrid and diagnosable

The working rebuild uses:

- dedicated Ore and ingot feeds;
- separate Titanium, Calcium, and Helium networks;
- two Wolfram networks because local volume is high;
- one bounded low-volume shared bus;
- direct inner loops for Ceramics → Synthetic Silicon and Chemicals → Acid;
- visible storage interfaces.

It is less visually pure than a universal bus, but it ran for hours under repeated buffer-emptying stress and remained traceable. The adopted rule is simple: a somewhat messy factory that can be debugged is better than a clean-looking factory that cannot.

## Three transport classes

1. **Local inner loops:** dominant-consumer or high-refill chains such as Ceramics → Synthetic Silicon, Tube → Inductor, HRS → Nozzle/Impeller, and heavy local Wolfram Plate demand.
2. **Buffered utilities:** expensive and reused products such as Chemicals, bounded Acid, and selected Helium products. Give each a visible capacity budget.
3. **Low-volume service goods:** Rotor, Stator, Nozzle, Impeller, Valve, Turbine, Pump, Battery, Supermagnet, Coil, and Accumulator.

Use demand-weighted rail load, not producer nameplate alone. A 1,600-unit destination depot can briefly request the entire rail during refill even when long-run consumption is small.

## Dominant-consumer bypass

When one consumer owns most flow, place it beside the producer and connect it directly. Expose only secondary demand to the shared bus.

```text
Producer ── direct ──> dominant local consumer
    └──── buffered bus ──> smaller or dispersed consumers
```

Faraday's Chemicals → Acid feed is the validated example. Battery plus Supermagnet Acid production requested roughly 95 Chemicals/min, while Supermagnet's separate direct Chemicals input was about 20/min. The bypass kept the dominant flow off the shared bus.

## Spatial rules

- Lay out main corridors before covering ground with Platforms.
- Put trunks at platform edges or in unplatformed easements.
- Give each district one logistics edge, one machine field, one player-facing output edge, and one open expansion edge.
- Keep high-rate inner loops short and direct.
- Put storage at every district boundary as both surge buffer and diagnostic checkpoint.
- When a service bus becomes too long, clone it as a separate acyclic segment rather than joining ends or adding shortcuts.
- Spread high-heat families across real Core districts; cosmetic spacing under one Core does not create capacity.

## Buffer diagnostics

- Source depot empty: production is the bottleneck.
- Source full, destination empty: routing or trunk capacity is the bottleneck.
- Destination full, campus starved: the local campus network is the bottleneck.
- Several destinations recover slowly together: shared-trunk contention is likely.

The normal warmed route can contain about 3,600 units per item: 400 at machine output, 1,600 at the source yard, and 1,600 at the destination yard. Formal campaign guarantees should normally credit only the destination's 1,600 so the upstream state is not hidden.

At the later planning checkpoint, V2 depots held later-stage parts rather than raw Ore. A mature intersite route was effectively triple buffered—machine-side V1, source-yard V2, destination-yard V2—but a newly unlocked output could still start at zero while only its inputs were warm.

## Player decision rules

- Offer owned V2 buildings as recipe options, not mandatory replacements. A V1/local recipe may win when the V2 path needs a distant input or creates more heat and freight.
- Keep Titanium Rod and Wolfram Plate off the world bus and out of export-space recommendations.
- End long-distance rails at storage yards; never use an internal factory graph as a through route.
- Split every recommendation by base and campus, including machine count, inbound flow, outbound flow, Core/heat impact, and reserved space.
- Use plain English and exact math. Avoid abstract layout language that cannot be translated into an in-game build.

## Rail teardown and unresolved orphan drones

The save produced floating cargo drones after a large teardown, and save/reload did not clear them. Their existence does not prove they caused the earlier deadlock; deleting destinations can itself orphan in-flight drones.

Current unresolved status: the archive never confirms that they were removed or that a repaired save was tested.

Safe handling boundary:

- stop producers/requesters and let traffic drain before removing destinations;
- recover reachable cargo with hold-E where possible;
- preserve an untouched manual save and a separate save-folder copy before external repair;
- use only a targeted stuck-drone repair on a copy with the game closed;
- validate save, full exit, reload, and ordinary rail flow before trusting a repaired copy.

## Superseded ideas

Do not reactivate these without new evidence:

- the original global Faraday loop and shortcut mesh;
- the Flowworks buffer-minimization study and five-output-depot bank;
- a second permanent HRS export depot as the default;
- a prebuilt separate Helium rail to Flowworks;
- campaign-scale Acid as a universal world-trunk utility;
- a third Faraday Supermagnet Furnace;
- the assumption that Grand Basin's five families require exactly five Cores.

The complete superseded calculations remain in the verified raw import for audit history, not current guidance.

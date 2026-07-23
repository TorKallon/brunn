Created: 2026-07-11
Updated: 2026-07-11

Related: [[Topics/StarRupture/Player save - sites and freight network|Sites and freight network]], [[Topics/StarRupture/Player save - operating model and factory architecture|Operating model and architecture]], [[Topics/StarRupture/Player save - progression and next builds|Progression and next builds]], [[Topics/StarRupture/Player save - exploration and combat|Exploration and combat]], [[Projects/RuptureOps/Player-derived product requirements|Player-derived product requirements]]

# Player save - play history

This is the canonical human-readable history of Rourke's Update 1 save. It preserves why the factories and plans changed, not just the latest answer. Current installed truth remains in [[Topics/StarRupture/Player save - sites and freight network|Sites and freight network]]; plans remain in [[Topics/StarRupture/Player save - progression and next builds|Progression and next builds]].

Machine-readable history: [rourke-update1-play-history__20260711.json](</Users/Shared/projects/ruptureops/models/rourke-update1-play-history__20260711.json>)

## Evidence boundary

The three supplied prompt dumps are preserved byte-for-byte in [the prompt-history import](</Users/Shared/projects/ruptureops/imports/player-prompt-history__20260711/README.md>). They overlap each other and the earlier ZIP transcript, so repeated statements are one canonical fact rather than multiple facts.

The prompts do not preserve original timestamps and came from overlapping conversation branches. The sequence below uses explicit narrative ordering. A historical checkpoint does not prove that the save has not advanced since.

## The save story

### 1. Landerworks: tangled, limited, and dependable

The first factory beside the lander began with dedicated forward and return rails. Shared return capacity was added later while forward routes stayed dedicated, creating difficult-to-read spaghetti. Because the plant stopped at relatively simple components and continued to work, it was left alone.

It became Landerworks: a buffered Rotor, Stator, and Applicator specialist. Its imperfect layout never starved Faraday, which established an enduring rule for this save: do not rebuild a stable specialist merely for visual purity.

### 2. Original Faraday: the delightful bus that failed

The first version of Base 2 kept Ore and ingots on dedicated routes but put Plate, Wire, Powder, and higher products on one shared loop. Its best moment was genuinely excellent: connect one machine to a single input, select an output, and return that output to the loop. The player found this fast, flexible, and unusually enjoyable.

As the loop grew, it slowed. Shortcuts and direct high-volume bypasses temporarily helped, then produced multiple opaque paths. Pulls appeared to stop even when a route looked valid. After roughly two hours of unsuccessful diagnosis, every machine in the factory was torn down.

The player suspected a game routing bug, but that was never proven. Hidden contention, request starvation, or a connection mistake remain possible. Floating cargo drones that survived save/reload were discovered after the teardown; they also remain an unresolved symptom, not established causation.

### 3. Rebuild experiment: from U-bus to hybrid

The initial rebuild concept separated Ore and ingots, gave Titanium/Calcium/Wolfram their own shape buses, and used a low-volume U-shaped central bus for advanced parts. Mixed-element recipes exposed the weakness: long extensions from multiple elemental buses still converged into spaghetti.

Two useful patterns survived:

- keep the high-rate Ceramics → storage → Synthetic Silicon path local, exposing only slack Ceramics elsewhere;
- place the Acid Pressurizer beside Chemicals so the dominant Chemicals flow goes directly to Acid while smaller Chemicals demand uses the bus.

The completed Faraday hybrid used a bounded low-volume bus, separate Titanium/Calcium/Helium networks, and two Wolfram networks. It was less visually pure but remained traceable and ran for hours while storage was repeatedly emptied as a stress test. The outbound yard grew to twelve Storage Depot v2 units.

### 4. Faraday operating and final retrofit

At one checkpoint Faraday had completed every turn-in the player could then manufacture, and there was no useful construction left until more blueprints were found. In a later branch, the player spent about three hours optimizing it until the factory ran at line speed.

The second Supermagnet Furnace was then installed. It is intentional burst/recovery capacity: Wolfram Plate was available, while synchronized Battery plus Supermagnet demand could still exceed the local Acid line. Grand Basin, not another disruptive Faraday rebuild, is the intended place to solve future campaign-scale Acid.

### 5. Pivot to exploration

Faraday, immediately north of CRRO “Warm Dawn,” was the launch point. The recorded goals were:

- unlock more blueprints;
- reveal more of the map;
- gather Quartz;
- solve food permanently;
- earn War Bonds for the Machine Gun;
- assess and prepare for the Forgotten Engine.

At that checkpoint the Engine was not confirmed complete, the Machine Gun had not yet been unlocked, and Goethite/Oil supply had not yet been established. Those are historical facts; their current state needs a live refresh.

### 6. Field lessons

- **Copperfield:** an accidental aggro exhausted ammunition, prevented disengagement, and ended in death. High-threat named sites need raid preparation and a retreat plan.
- **First Geo Scanner:** the first activation was unexpectedly difficult and produced two deaths before success. The first respawn landed at a nearby claimed base with no replacement weapons or materials, forcing a walk back to Landerworks. On the second attempt the scanner fire base's turrets had not been powered; after the player was overwhelmed, corruption disabled the base and its claimed Regeneration Chamber, sending the next respawn all the way to the starting launcher. The successful attempt used a fully built-out fire base with every turret powered. The resulting automated defense was a strongly positive payoff—the turrets “singing” against the enemy turned the frustrating setup into an excellent learning experience.
- **Remote recovery:** proximity alone does not make a claimed base a useful respawn. Remote staging sites need replacement weapons, ammunition, healing, and rebuilding material, preferably in a safe cache outside the contested base's corruption failure domain.
- **Scanner food loot:** the completed site appeared to provide two unusually good food-related rewards or stacks, possibly with a quantity of 20. The original dictated wording is ambiguous, so exact items/counts remain unresolved.
- **Combat style:** the player explicitly wants conservative plans suitable for someone who is not strong at twitch FPS play. Recommended gear must not be treated as owned gear.
- **Forage lake:** a rail and zipline cross one productive lake without Platforms, and substantial plant spawning continued. This disproves a blanket “any nearby structure stops respawns” rule but says nothing definitive about Platforms.
- **Food inventory:** Hydrobulb was gathered most often, then Polufruit, then Prickler. Glowcaps were being hoarded; the food-strategy decision recorded 157, all lifetime cave harvest, but the current quantity is unknown.
- **Figler:** the player reported unlocking it and then having nothing left. Separately, the structured model lists 30 Prickler plus 100 Data Points as research inputs and the finished Figler as a later Food Station craft using 3 Prickler.
- **Yellowstone:** it was deferred as an exceptionally difficult, non-urgent objective; no visit is recorded.

### 7. Planet-scale planning branch

The selected next sites were Flowworks between Landerworks and Faraday and Grand Basin immediately east of Mythic Cry. Grand Basin was envisioned as several logical campuses, potentially spread beyond the flat pad to reduce heat, concentrated Vermin attacks, and Meteorite Core pressure.

Later prompts recorded a pure Titanium source and merged pairs of Helium and Sulfur sources whose serving rails delivered only about one source's flow. The player called the desired upgrade “v4,” but the verified Update 1 table calls the 480/min player-facing tier Rail v3. Preserve the bottleneck; recheck the live rail tier before naming the fix.

Ore Excavator v2 was not owned at that checkpoint. Long-distance rails terminated at storage yards. Titanium Rod and Wolfram Plate were hard rejects for world-bus/export space. A separate ammunition/building-material Cargo service for temporary firebases and Geo Scanner assaults was being considered, not reported as built.

## Durable play preferences

- Optimize finite campaigns and recovery during active play, not only continuous steady state.
- Do not substitute giant storage banks for adequate production.
- Strategic stockpiling is smart; AFK-only completion feels cheesy.
- Below roughly level 12, target under one hour per turn-in and allow two hours only rarely.
- For later final campaigns, one to two hours is preferred; an occasional three to four hours is tolerable.
- A few modest specialist factories are fun. Rebuilding the same large dependency chain repeatedly is tedious.
- Cheap drills may be overbuilt for cold-start recovery when duplicating a large hot processing chain would be worse.
- Offer V2 recipes as choices, not automatic answers; distance and onsite resources can make a V1/local path better.
- Split plans by base and campus, and use named geography before coordinates.
- Use plain English, exact counts, visible math, and explicit assumptions.
- Preserve reopenable visual plans and the reasoning behind changes.

## Resume questions

Before the next live play recommendation, refresh:

1. Corporation levels and partial reputation.
2. Current rail tier and the pure Titanium/merged Helium/Sulfur bottlenecks.
3. Ore Excavator v2, Machine Gun, weapons, mods, and blueprint ownership.
4. Forgotten Engine and Redleaf completion.
5. Whether Flowworks, Grand Basin, or firebase Cargo construction began; the first temporary Geo Scanner fire base is confirmed built, but no reusable Cargo service is confirmed.
6. Whether the orphan drones were removed.
7. Current food, ammunition, Quartz, War Bonds, and Glowcap stock.
8. Which Geo Scanner was completed and the exact identity/count of its two reported food rewards.

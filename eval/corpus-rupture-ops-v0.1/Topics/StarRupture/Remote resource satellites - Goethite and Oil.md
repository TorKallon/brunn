Created: 2026-07-11
Updated: 2026-07-11

Related: [[Topics/StarRupture/Player save - sites and freight network|Sites and freight network]], [[Topics/StarRupture/World geography and named locations|World geography]], [[Topics/StarRupture/Player save - progression and next builds|Progression]]

# Remote resource satellites - Goethite and Oil

## Principle

Goethite and Crude Oil are compact at extraction and expand heavily during processing. Move the raw resource through one item-specific Cargo link, then process it beside Grand Basin consumers. Do not spend world-trunk capacity moving expanded intermediates from a remote outpost.

## Goethite

Preferred permanent source: the safer northwestern triple at approximately `1393–1464,1838–1891`, northwest of Mythic Cry/Grand Basin.

Adopted plan after Engine access and Selenian 9:

- build all three Laser Drills on the first permanent visit;
- locally merge them into one Goethite-only Cargo Dispatcher;
- receive 45 raw Ore/min through one Receiver;
- place one 1,600-unit Storage Depot v2 immediately after the Receiver;
- begin with only the Pyro Forges current consumers require;
- charge the receiver buffer before enabling a processor that exactly matches source flow.

A geographically separate fourth drill is a strict empty-buffer fallback. Because it cannot join the three-node field directly, it needs its own Dispatcher/Receiver pair before the two received streams merge locally.

The closer west field around `1252–1472,2950–3282` overlaps scanner/hostile envelopes and is not the preferred first permanent outpost.

## Oil

Preferred first source: approximately `1234,3760`, west/southwest of Mythic Cry and Grand Basin.

Adopted plan after Future Health 11:

- build one Oil Extractor;
- use one Crude-only Dispatcher/Receiver pair;
- receive 10 Crude/min into one V2 buffer;
- feed one GB-A Refinery;
- reserve, but do not initially build, a second Oil pair and second Refinery.

One Refinery consumes the full 10 Crude/min source plus large Titanium and Calcium feeds, so duplicating extraction implies duplicating the expensive ore-fed refining line. The second western Oil marker around `1351,4473` is a separate site, not a same-pad extension.

## Receiver-yard rules

- Goethite and Oil may share a physical receiver yard.
- They never share a Cargo link, rail lane, or storage unit.
- One Update 1 Dispatcher selects one Receiver.
- Keep each raw interface legible and item-specific.
- Allow the incoming buffer to fill before enabling exact-rate downstream processing.
- Add fallback extraction only when measured campaign recovery requires it.

## Version boundary

The imported remote-resource calculations used the older July 8 analysis database and sometimes described it as build `23761620`. Preserve their decisions, but rerun the exact late-campaign quantities against the canonical RuptureOps snapshot before publishing them in the app.

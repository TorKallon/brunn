# Star Rupture Game Analysis import

This directory records the lossless import and reconciliation of
`Star Rupture Game Analysis.zip`, supplied on 2026-07-11.

## Integrity and provenance

- Original ZIP: 21,674,717 bytes
- ZIP SHA-256: `1a1c1ef0ae00905ced37f1221ab22c4b9268f2ebe17b7516e6c9cac67d7f6fa3`
- Archive entries: 123, including directories
- Packaged files: 115 manifest entries plus `CHECKSUMS.sha256`
- Integrity: `unzip -t` passed and every packaged SHA-256 passed
- Archive data retrieval time: `2026-07-08T19:01:37Z`
- Archive source-site version: not recorded
- Archive game-data build claim: not recorded reliably
- Latest public build observed by the archive: Update 1 Hotfix `0.2.8`,
  Steam build `23761620`

The original ZIP and a verified extracted tree are retained under `raw/`.
That directory is intentionally ignored by Git because it contains a duplicate
third-party data corpus, personalized save context, generated binaries, and a
legacy SQLite database. `MANIFEST.csv` and `CHECKSUMS.sha256` remain tracked so
the complete local package can always be audited.

## Version correction

The archive repeatedly describes its numeric data as build `23761620`. That is
too strong. `23761620` was the latest public executable observed, not a proven
build identity for every source value.

RuptureOps retains the stronger two-axis boundary:

- source/game-data claim: Update 1, internal version `35118973`, dated
  2026-04-09 and contextually mapped to Steam build `22674441`;
- canonical source capture: SRDB `2.3.4`, captured 2026-07-11;
- latest public executable observed at capture: Hotfix `0.2.8`, Steam build
  `23761620`.

Imported numeric reports remain legacy calculations until rerun against the
canonical snapshot
`ea-u1-gv35118973__srdb-2.3.4__20260711T050230Z`.

## Reconciliation result

The archive's 468 items, 191 public buildings, 234 recipe definitions, 77
Corporation levels, 121 export offers, 15 upgrades, 3,259 map markers, 3,094
loot rows, and official-news subset are semantically duplicated by the richer
RuptureOps corpus. They were not installed as a competing snapshot.

Unique material promoted from the package:

- the player save baseline in `models/rourke-update1-save-baseline__20260711.json`;
- six imported Ore Excavator v2 recipe/producer associations in
  `sources/recipe-producer-overrides.json`;
- explicit consumable-effect assumptions in `sources/consumable-effects.json`;
- nine player-specific site/geography assets under the StarRupture vault;
- consolidated player-save, architecture, geography, progression, exploration,
  food, and remote-resource notes in the StarRupture vault.

The import comparison also exposed and corrected two normalization defects:
the `airlock` item had been overwritten by a building-shaped detail response,
and Training Corporation retained a literal `$undefined` index.

## Superseded material

The complete 18-file `Flowworks Campaign Sizing - Levels 8 to 15` family is
audit history only. It over-rotated toward buffers and was explicitly rejected.
The current staged Flowworks plan reaches a level-10 cell with one Stabilizer,
one Nozzle, one Impeller, two Valves, one Turbine, and two Pumps; the second
Stabilizer comes later, and HRS v2 is an evidence-triggered recovery option
rather than an initial build.

The archive's SQLite database, source snapshot JSON, core delivery CSVs,
workbook, refresh script, and query script are preserved only under `raw/`.
The active RuptureOps pipeline and database remain authoritative.

## Supplemental prompt history

Three later-supplied user prompt dumps overlap this package's curated prompt
export but also contain unique scaling, iOS-product, exploration, food, combat,
and Cargo-planning history. Their exact texts and dedupe audit are preserved at
`imports/player-prompt-history__20260711/`; the original package remains
unchanged.

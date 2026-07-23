Created: 2026-07-11
Updated: 2026-07-11

Related: [[Active projects]], [[INDEX|Shared knowledge index]], [[Home]], [[Topics/StarRupture/StarRupture|StarRupture]]

## Status

Active. Repository and versioned data foundation are bootstrapped. The prior research package plus all three user-authored prompt dumps have been reconciled into current save state, play history, and evidence-backed iOS requirements. The first native feature is selected: a phone-first Ruptura Watch Session with an always-readable countdown, enemy-return warnings, and phase-aware opportunity guidance.

## Repo

`/Users/Shared/projects/ruptureops`

Branch: `main`

No remote is configured yet.

## Purpose

RuptureOps is an unofficial iOS companion for StarRupture. It should make complex game planning useful at hand while playing rather than reproduce a static wiki.

The product can grow across:

- factory and recursive production planning;
- machine, power, and Base Core capacity calculations;
- corporation, research, blueprint, and Development Station progression;
- map, resource, loot, and point-of-interest reference;
- saved plans and player-specific state;
- Fire Wave and other time-sensitive helpers.

## Current foundation

- Versioned Early Access Update 1 research and mechanics notes live under [[Topics/StarRupture/StarRupture|StarRupture]].
- The repo contains reproducible capture, normalization, SQLite, query, and validation tooling.
- Game-build and source-snapshot versions remain separate.
- The first corpus includes items, buildings, recipes, progression, map markers, loot, weapons, official announcements, and saved planning scenarios.
- The raw mirror and generated SQLite database moved out of the synced vault and into the local repo, where they are ignored by Git.
- The verified `Star Rupture Game Analysis.zip` import is preserved losslessly under `imports/star-rupture-game-analysis__20260708T190137Z/raw/` and reconciled against the richer canonical corpus.
- All three supplied prompt-history dumps are checksummed and tracked under `imports/player-prompt-history__20260711/source/`; their overlapping facts are promoted only once.
- Player-specific state is captured in `models/rourke-update1-save-baseline__20260711.json` and the linked StarRupture vault notes.
- Factory evolution, incidents, exploration checkpoints, and durable preferences are captured separately in `models/rourke-update1-play-history__20260711.json` and [[Topics/StarRupture/Player save - play history|Player save - play history]].
- [[Projects/RuptureOps/Player-derived product requirements|Player-derived product requirements]] translates actual play into requirements for state evidence, campaign timing, alternative recipes, site ownership, diagnostics, expeditions, food, and saved visual plans.
- [[Projects/RuptureOps/Rupture cycle timer - research and product design|Rupture cycle timer - research and product design]] captures the selected first feature, current timer/game evidence, native alert strategy, visual hierarchy, and exact-sync path.
- Nine original site/geography assets are retained in `Topics/StarRupture/assets/sites/`.
- The import exposed and corrected the `airlock` item/building normalization collision and preserved six medium-confidence Ore Excavator v2 purity associations.

## Product boundaries

- The app is unofficial and must not imply endorsement by Creepy Jar.
- Hidden, prototype, and stale play-test content stays out of player-facing defaults.
- Numerical results must expose their version boundary when it matters.
- The current source publishes no explicit reuse license. Keep the repo and normalized corpus private until reuse permission or a different licensed-data strategy is established.
- Detailed implementation truth belongs in the repo; the vault retains product direction, decisions, and durable game interpretation.

## Current focus

Build the smallest trustworthy native Watch Session. It should use a versioned 54-minute community/game-data model, manual fire-wave anchoring, explicit prediction confidence, a large next-event countdown, a conservative ten-minute lower-threat recovery window, and distinct enemy-return and Ruptura warnings. Keep enemy and gathering events separately versioned so later live validation can move them without rewriting phase logic.

## Next step

Validate several Hotfix 0.2.8 cycles in solo, hosted co-op, and dedicated-server play, especially fire-wave impact, ordinary enemy return, cave closure, opportunity transitions, warning cues, pause, and save/reload behavior. Then select the first visual direction and scaffold the focused SwiftUI Watch Session without disturbing the existing data-pipeline work.

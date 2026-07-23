# Player prompt history import

This directory preserves the three user-authored StarRupture prompt dumps
supplied on 2026-07-11. Together they contain the direct history of the
player's bases, factory rebuild, exploration, combat, planning preferences,
and the origin of the RuptureOps iOS idea.

It also records the requested context transfer from Codex thread
`019f5263-8bae-7d10-b944-0fd4a543afb9`. That thread had already isolated the
clean pre-migration StarRupture context from upstream thread
`019f4f6d-169c-70e2-8de4-81722b41eee9`, selected the RuptureOps name, and
bootstrapped the repo/vault split. Migration, sync, handoff, and activation
noise remains excluded as requested.

## Integrity

| Source | Bytes | POSIX lines | SHA-256 |
|---|---:|---:|---|
| `prompt-dump-01-factory-history.txt` | 16,533 | 225 | `a6534f50df6a09c98d6e3465488b60a60e5452a47cc2f63251df049c1cdc4be1` |
| `prompt-dump-02-scaling-and-app.txt` | 25,263 | 251 | `adde9855a1c3e439a02b6ccb243779ab29ed6a40602d15316caa369e1d90ec6d` |
| `prompt-dump-03-exploration-and-combat.txt` | 16,411 | 228 | `dc259be82ef89c908785b59644fa1f27f2be914c43f95f4f35351f94e1803a40` |

The copies under `source/` are byte-identical to the supplied attachments.
Unlike third-party game-data mirrors, these small user-authored sources are
tracked because the user explicitly asked to retain the complete conversation
history. The repository must remain private under the existing data boundary.

## Deduplication

The prompt dumps overlap one another and the verified July 8 research
package. They are preserved intact as provenance, but they do not create a
second game-data snapshot or three competing versions of the save.

- Dump 01 is a formatting variant of the factory-planning sequence already in
  the prior package. It adds no new conversation content.
- Dump 02 shares the same opening, then adds the app origin, later-game
  scaling preferences, installed-state corrections, freight conventions, and
  Cargo Dispatcher ideas.
- Dump 03 shares the factory opening, then adds the exploration pivot,
  Copperfield death, food observations, low-twitch combat constraint, and
  unresolved weapon/Yellowstone questions.

The exact overlap decisions and evidence rules are recorded in
`semantic-audit.json`. Direct player statements were promoted once into the
play-history model and canonical vault notes. Quoted assistant recommendations
were not reclassified as player observations, questions were not treated as
answers, and plans were not mistaken for completed construction.

## Canonical outputs

- `models/rourke-update1-save-baseline__20260711.json` remains the best current
  installed-state model.
- `models/rourke-update1-play-history__20260711.json` records historical
  checkpoints, incidents, and durable player preferences.
- `Player save - play history.md` is the human-readable gameplay timeline.
- `Player-derived product requirements.md` turns real play evidence into iOS
  product requirements without declaring every idea an MVP feature.

The earlier ZIP import remains the canonical source for its screenshots,
generated reports, and the more complete first factory-planning transcript.

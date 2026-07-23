Created: 2026-04-18 20:27 PDT
Updated: 2026-04-18 20:27 PDT

## Goal
Clean up the `Scanned Documents` Google Drive folder without losing the ability to detect new/unprocessed files reliably.

## Preferred Drive structure
Rourke prefers a flatter structure.

Target shape inside `Scanned Documents/`:
- `From Evernote/` (temporary legacy bucket during migration)
- `2005/`
- `2006/`
- `2007/`
- ...
- `2025/`
- `Unknown Date/`

End state:
- `From Evernote/` should disappear once the reorganization is complete and verified.

## Date assignment rules
For each file, choose the best year using this order:
1. Date parsed from filename when it looks trustworthy.
2. Pre-2026 file metadata date when it looks plausible and not just a recent upload/migration timestamp.
3. Otherwise `Unknown Date/`.

Store both:
- derived date/year
- date confidence (`filename`, `metadata`, `unknown`)

## How tracking should work
Do not use folder placement as the source of truth for processed vs unprocessed.
Use the Google Drive file ID as the canonical key.

Maintain a local Metis state/index keyed by Drive file ID, for example:
- `fileId`
- `name`
- `driveLink`
- `mimeType`
- `originalFolderPath`
- `currentFolderPath`
- `derivedDate`
- `derivedYear`
- `dateConfidence`
- `firstSeenAt`
- `lastSeenAt`
- `driveModifiedTime`
- `processingStatus`
- `notePath`
- `reviewNeeded`
- `ocrConfidence`

## How to find new or unscanned files
Each indexing run should:
1. List files under `Scanned Documents`.
2. Compare returned Drive file IDs against the saved local state/index.
3. Mark files as:
   - `new` if the file ID has never been seen before
   - `changed` if the file ID exists but modified time or name changed
   - `known` if already seen and unchanged
4. Treat `new` and `changed` files as candidates for processing/reprocessing.

That means:
- moving a file between folders does not break tracking
- renaming a file does not break tracking
- reorganizing from `From Evernote/` into year folders does not lose process state

## Processing states
Suggested states:
- `indexed`
- `downloaded`
- `note_created`
- `review_needed`
- `summarized`
- `promoted`
- `ignored_non_document`

## Recommended rollout
### Phase 1
- Keep current processing logic.
- Add/clean up the Drive state file keyed by file ID.
- Make sure every generated note already records the Drive file ID.

### Phase 2
- Run a dry analysis across `From Evernote/`.
- Compute proposed year destinations and confidence.
- Produce a move report before touching Drive.

### Phase 3
- Move only high-confidence files into year folders.
- Leave ambiguous files in `From Evernote/` or place them in `Unknown Date/`.
- Re-index and confirm links/state remain intact.

### Phase 4
- Process remaining ambiguous files.
- Empty out `From Evernote/`.
- Delete `From Evernote/` only after verification.

## Why this answers the tracking problem
The key insight is:
- organization is for humans
- Drive file ID state is for the machine

So we can make Drive cleaner without losing the ability to answer:
- what is new?
- what has never been processed?
- what changed since the last run?
- which files still need review?

## Recommendation
Proceed with:
- flat year folders beside `From Evernote/`
- `Unknown Date/` as the fallback bucket
- local file-ID-based state tracking as the source of truth
- deletion of `From Evernote/` only after a verified migration pass

Created: 2026-04-18 12:50 PDT
Updated: 2026-04-20 16:30 PDT

## Goal
Process historical documents from Google Drive without bloating the vault.

## Related
- [[Projects/Metis/Metis|Metis]]
- [[Projects/Metis/UpNote migration runbook|UpNote migration runbook]]
- [[Active projects]]
- [[Vault rules]]

## Storage rule
- born-digital, information-dense PDFs should be retained in Obsidian for deeper AI analysis
- scanned receipts and scanned documents should stay in Drive, with Obsidian storing extracted text, structured notes, and the Drive link

## Recommended workflow
### Target intake flow for new scans
Best steady-state workflow:
1. Rourke drops new scans into `Scanned Documents/Incoming` in Google Drive.
2. Aether treats that folder as the intake queue.
3. The pipeline indexes only that intake area, downloads the new files locally, and classifies/OCRs them.
4. The pipeline creates or updates Obsidian notes from the download report.
5. Originals are then moved in Google Drive from `Incoming` to the right long-term Drive location.
6. The corresponding Obsidian notes land in `Records/Drive Inbox/` with extracted text, structured facts, Drive links, and review status.
7. Important notes can then be promoted into long-term homes like `Records/Finance/`, `Records/Health/`, `Records/Taxes/`, or linked into project/trip/topic notes.

Important nuance: the original scan usually should **not** land inside Obsidian. For scanned documents, the desired end state is usually:
- original file stays in Google Drive
- Obsidian gets the processed note, extracted text, metadata, and source link

The manifest classification is only a first-pass hint. The real retain-vs-link decision happens during the download step after local PDF/text analysis runs on the actual file.

## Commands
Index:
```bash
cd /Users/Shared/projects/metis
node scripts/scanned-docs.js index-drive \
  --root-id "DRIVE_FOLDER_ID" \
  --out ./out/scanned-docs/manifest.json \
  --account "you@example.com"
```

Download + classify:
```bash
cd /Users/Shared/projects/metis
node scripts/scanned-docs.js download \
  --manifest ./out/scanned-docs/manifest.json \
  --out-dir ./out/scanned-docs/downloads \
  --account "you@example.com" \
  --limit 25
```

This now writes `./out/scanned-docs/downloads/download-report.json` with:
- storage decisions
- PDF text-layer analysis when available
- extracted text for supported files
- OCR text for scanned PDFs and images when local OCR succeeds
- stronger Gemini-based LLM OCR when `GEMINI_API_KEY` or `GOOGLE_API_KEY` is configured

Create note stubs:
```bash
cd /Users/Shared/projects/metis
node scripts/scanned-docs.js note-stubs \
  --download-report ./out/scanned-docs/downloads/download-report.json \
  --vault-dir /Users/aether/obsidian/notes
```

## What happens now
- born-digital dense PDFs can be copied into `Records/Retained PDFs/`
- scanned receipts and scanned documents stay in Drive
- note stubs land in `Records/Drive Inbox/`
- the notes include the Drive link, storage decision, extracted text, OCR output when available, plus a confidence/review-needed signal so weak OCR cases stand out faster

## Desired automation state
The intended end-state is:
- `Scanned Documents/Incoming` acts as the capture queue
- Aether processes that queue on demand or on a scheduled run
- originals get routed to the correct permanent folder in Drive
- Obsidian ends up with the finished note and the useful extracted knowledge
- manual intervention is mostly reserved for ambiguous classifications, weak OCR, or documents that deserve promotion into richer long-term notes

## Current automation wiring
As of 2026-04-19, there is a wrapper script at `/Users/Shared/projects/metis/scripts/process-incoming-scans.js`.

Current behavior:
- checks `Scanned Documents/Incoming`
- if empty, exits quietly
- if files are present, runs the Metis intake pipeline
- creates/updates Obsidian note stubs from the download report
- moves the processed original Drive items out of `Incoming`
- archives them into the year folder at the root of `Scanned Documents` (for example `2026`)

That archive step is the current practical version of “move to the right permanent location.” It is a good default for now, and we can add smarter category-specific routing later if we want.

## Practical reminder
This is still a staged workflow, but OCR is now wired in.
If you want the stronger model path, set `GEMINI_API_KEY` or `GOOGLE_API_KEY` first. The current OCR setup uses `gemini-2.5-pro` as the primary model and can retry weak cases with `gemini-3.1-pro-preview` as a stronger fallback. If a scan is especially messy, rotated badly, or visually degraded, the OCR output may still need cleanup.

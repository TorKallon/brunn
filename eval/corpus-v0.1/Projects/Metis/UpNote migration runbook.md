Created: 2026-04-18 12:35 PDT
Updated: 2026-04-18 14:24 PDT

## Goal
Move the UpNote library into an Obsidian staging vault with minimal manual work, while preserving the handful of table-heavy or formatting-sensitive notes as well as practical reality allows.

## Storage rule during migration
- born-digital, information-dense PDFs should be retained directly in the vault
- scanned receipts and scanned document PDFs should normally be Drive-linked rather than duplicated into the vault

## Current migration assumption
For Rourke's existing UpNote library, assume the document attachments are already in Google Drive.
That means the UpNote migration should not upload duplicate copies to Drive again during this pass.
Run the migration without `--drive-upload-parent` unless we later decide to build a matching/linking pass against the existing Drive archive.

## What the helper now does
- pairs HTML and Markdown notes recursively
- converts HTML notes into Obsidian-friendly Markdown
- preserves complex tables as raw HTML when Markdown tables would break
- copies normal attachments into local note asset folders
- rewrites resolvable internal note links
- automatically keeps born-digital dense PDFs in the vault
- automatically uploads scanned PDFs to Drive when a Drive destination folder is configured
- creates Obsidian companion notes for Drive-routed PDFs with extracted text or OCR text plus the Drive link
- can use Gemini-based LLM OCR automatically when `GEMINI_API_KEY` or `GOOGLE_API_KEY` is configured

## Run it
Base run:
```bash
cd /Users/Shared/projects/metis
npm run upnote:migrate -- \
  --html-dir "/path/to/UpNote_HTML_Export" \
  --md-dir "/path/to/UpNote_Markdown_Export" \
  --output-dir "/path/to/obsidian-staging-vault"
```

Fully automated scanned-PDF routing:
```bash
cd /Users/Shared/projects/metis
npm run upnote:migrate -- \
  --html-dir "/path/to/UpNote_HTML_Export" \
  --md-dir "/path/to/UpNote_Markdown_Export" \
  --output-dir "/path/to/obsidian-staging-vault" \
  --drive-upload-parent "DRIVE_FOLDER_ID" \
  --drive-account "you@example.com"
```

## What to review after the run
Open:
- `.../_metis/upnote-migration-report.md`
- `.../_metis/upnote-migration-report.json`

Review first:
1. notes with preserved complex tables
2. unresolved internal links
3. notes missing an HTML or Markdown pair
4. any PDF whose Drive upload failed and fell back to a local companion note

## Important nuance
This is much more automatic now.
The helper now attempts local OCR for image-heavy PDFs too, and can use Gemini LLM OCR when a key is configured. Recommended default model: `gemini-2.5-pro`. The remaining limitation is quality, not missing plumbing: badly degraded scans can still need cleanup after OCR.

Also, no-upload is not the same thing as automatic existing-Drive-link resolution.
If we skip uploads for the current UpNote library, the migration will avoid duplicates, but a later matching pass may still be needed if we want companion notes to point at the exact pre-existing Drive files.

Created: 2026-04-18 11:27 PDT
Updated: 2026-04-18 PDT

## Decisions made
- Metis is the umbrella project for Obsidian knowledge-base design plus UpNote migration.
- Obsidian should become the long-term primary knowledge base.
- The migration should be treated as a staged systems project, not a one-shot import.
- Existing LLM-output notes are useful seed material, but the target state is a broader lifelong knowledge base.
- A lightweight structure is preferred over a heavy PKM framework.
- For scanned paper bills/documents, `scanned` should be treated as workflow evidence that the paper has already been dealt with, not necessarily as explicit payment evidence from the document itself. Metis should track workflow/archive status separately from document-derived payment status.
- Rourke is fine with Obsidian Sync Plus if needed. For Metis, treat Obsidian Sync limits as hard operational constraints. Design against Sync Plus limits, especially 10 GB account-wide storage, 200 MB max file size, 10 synced vaults, 12 months version history, and the fact that attachments/version history count toward storage.
- Default document-storage rule for Metis: keep original scanned files in Google Drive and keep links in Obsidian rather than storing duplicate originals in the synced vault. Allow selective exceptions for high-value deep-exploration sets, especially taxes, where keeping local working copies or curated retained copies may be worth it.
- More specific operating rule: born-digital, information-dense PDFs should be retained in the vault for deeper AI analysis; scanned receipts and scanned documents should stay in Drive, with Obsidian holding extracted text, structured notes, and the source link.

## Likely decisions pending confirmation
- top-level folders will probably include Inbox, Journal, Projects, Areas, Topics, References, Briefings, Archive, and optional Assets
- UpNote migration will likely use backup + HTML + Markdown export with staging before live import
- recurring subjects should be promoted into evergreen topic pages over time

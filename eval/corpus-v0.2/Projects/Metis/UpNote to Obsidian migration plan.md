Created: 2026-04-18
Updated: 2026-04-18 13:18 PDT

# UpNote to Obsidian migration plan

## Related notes
- [[Projects/Metis/Metis|Metis]]
- [[Projects/Metis/Obsidian knowledge base master plan|Obsidian knowledge base master plan]]
- [[Projects/Metis/Obsidian vault architecture plan|Obsidian vault architecture plan]]
- [[Projects/Metis/UpNote migration runbook|UpNote migration runbook]]

## Best approach
Use a **staged migration**, not a one-shot import:

1. **Create and keep an immutable UpNote backup** from desktop, with **attachments included**.
2. From the same frozen library, make **two full exports**: **HTML** and **Markdown**.
3. Convert those exports into a **new staging Obsidian vault** using a purpose-built converter such as **jvhaarst/UpNote_To_Obsidian**.
4. Validate a representative sample, then rerun the same process for the full library.
5. Keep the original UpNote backup and export folders until you have lived in Obsidian for a while.

This is the best balance of **format fidelity + attachment safety + Obsidian searchability**.

## What UpNote officially exports
UpNote officially supports export to:
- **PDF**
- **Text (.txt)**
- **HTML**
- **Markdown (.md)**

For **HTML** and **Markdown** exports, UpNote creates an export folder and puts images/attachments in a **`Files`** subfolder.

UpNote desktop backup also supports:
- local automatic/manual backups
- **backup attachments**
- **export backups to Markdown**

## Why not use just one export format?
- **Markdown export only**: best for plain text and metadata, but community converters note it can **lose code block formatting** and **flatten/break tables**.
- **HTML export only**: best for rich formatting, images, and tables, but it is weaker for structured metadata and some tools report **missing note links** and **missing folder hierarchy**.
- **PDF/TXT**: useful only as archival fallback, not as a searchable Obsidian vault.

## Recommended workflow
### 1) Freeze the source
- Do the migration from a desktop copy of UpNote.
- Avoid editing notes during export/conversion.

### 2) Make a safety backup first
In UpNote desktop:
- run **Backup now**
- enable **Backup attachments**
- if available, enable **Export backups to Markdown**

Archive that backup read-only.

### 3) Create matching full exports
From the same note state:
- export **all notes to HTML**
- export **all notes to Markdown**
- keep them in separate folders
- for HTML, use **Expand All** if prompted

### 4) Convert into a staging vault
Primary recommendation:
- use **UpNote_To_Obsidian** on the **HTML + Markdown** exports

Why:
- preserves rich content better
- produces real Markdown notes for Obsidian search
- copies attachments locally into the vault
- can preserve metadata like dates/categories from the Markdown export

### 5) Validate before cutover
Spot-check at least:
- note count matches expectations
- attachments open correctly
- tables render acceptably
- code blocks remain code blocks
- tags/dates/notebook mapping look right
- Obsidian search finds unique phrases from old notes
- internal note links work where you rely on them

### 6) Keep a fallback path
If **internal note links** or **nested notebook hierarchy** matter a lot, test **Jimmy** against the native UpNote backup too. Its docs say the backup route can preserve:
- attachments/resources
- labels/tags
- note links
- notebook/folder hierarchy
- rich text

That makes it a strong fallback, and possibly the better choice if link/hierarchy fidelity matters more than table/code-block fidelity.

## Biggest risks
1. **Complex tables**
   - UpNote supports merged cells, cell colors, and rich content inside tables.
   - Obsidian Markdown tables are much simpler.
   - Expect some manual cleanup for advanced tables.

2. **Internal note links**
   - UpNote uses its own note-link system.
   - Export-based workflows may not convert every note-to-note link cleanly.

3. **Notebook hierarchy vs tags/folders**
   - Some converters map notebooks to folders, others to tags/frontmatter.
   - Verify the structure matches how you want to work in Obsidian.

4. **Formatting edge cases**
   - Inline colors, special embeds, and app-specific styling may not survive.

5. **Filename/path issues**
   - Large libraries can expose duplicate titles, slashes, or attachment path quirks.

## Safest practical rule
- **Backup first**
- **Convert into a fresh staging vault**
- **Validate a sample before full cutover**
- **Keep UpNote read-only for a while after migration**

## Bottom line
For a large personal knowledge base, the safest primary route is:

**UpNote backup for safety + HTML export for fidelity + Markdown export for metadata + convert into a staging Obsidian vault.**

Major risks are **complex tables**, **internal note links**, and **structure mapping**. Keep the native backup so anything missed can be recovered later.

## Sources
- UpNote Help: export notes
- UpNote Help: write with Markdown
- UpNote Help: automatic notes backup
- UpNote Help: links / tags / tables
- Jimmy UpNote format docs
- `jvhaarst/UpNote_To_Obsidian` README

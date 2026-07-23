Created: 2026-04-18 11:37 PDT
Updated: 2026-04-18 11:40 PDT

## Goal
Define the canonical pipeline for turning scanned documents into trustworthy Obsidian knowledge objects.

## Design principles
- Google Drive keeps the original file and remains the source of truth.
- Obsidian stores the knowledge layer, not just the binary artifact.
- Raw OCR and cleaned interpretation stay separate.
- Structured extraction should be evidence-backed.
- The system should optimize for trust first, then cost, then convenience.

## Recommended pipeline
### 1. Ingest
Input:
- PDF or image scan from Google Drive

Keep:
- original Drive file untouched
- stable Drive link captured immediately

### 2. OCR and layout extraction
Run OCR first, not a multimodal LLM first.

Desired outputs:
- raw OCR text
- page boundaries
- block/line structure if available
- confidence scores if available
- layout cues for tables when possible

Why:
- cheaper at scale
- easier to audit
- better as a system of record

### 3. Document classification
Classify the document into a type before extracting fields.

Examples:
- utility bill
- medical bill
- bank statement
- insurance notice
- tax form
- legal letter
- contract
- receipt / invoice
- identity / official record
- unknown

This determines which extraction schema to use.

### 4. Structured extraction
Use a text LLM over OCR output to produce structured JSON and a draft Obsidian note.

For bills, target fields like:
- issuer
- document date
- service period
- account number
- masked account number if only partial number appears
- amount due
- amount paid
- due date
- payment status
- autopay status
- itemized charges
- notes / warnings
- confidence per field
- evidence snippet per field

Important rule:
The LLM should return structured data first.
The human-readable note should be generated from the JSON, not from freeform prose.

### 5. Validation layer
Apply rules before the note is trusted.

Examples:
- totals add up or at least reconcile within tolerance
- due date parses correctly
- account number matches expected shape if known
- itemized rows sum to total if possible
- payment status only accepted if explicitly evidenced

### 6. Confidence routing
Route documents into one of three buckets:
- **High confidence**: safe to create final note automatically
- **Medium confidence**: create draft note and flag for review
- **Low confidence**: hold for manual review

### 7. Obsidian output
Create a document note from the validated structured output.

Recommended note sections:
- Summary
- Key facts
- Payment status
- Itemization
- Source
- Confidence / review notes
- Raw OCR

## Workflow status vs payment status
In Rourke's paper workflow, a scanned document usually means the paper has already been dealt with and then trashed. That is useful historical context, but it is not the same thing as what the document explicitly says.

So the system should track two separate concepts:
- **workflow/archive status**: for example `scanned`, `handled`, `archived`
- **document-derived payment status**: what the bill or statement explicitly indicates about payment or autopay

Recommended rule:
- if the document entered Metis via Rourke's scan-and-trash workflow, default workflow status to something like `scanned_handled`
- still extract payment/autopay status from the document text separately
- if no explicit payment evidence appears, payment status can remain `unknown` while workflow status still records that the paper was already dealt with

## Payment-status rules
This is one of the easiest places to get false confidence, so be strict.

### Allowed values
- `paid`
- `autopay_scheduled`
- `autopay_enabled`
- `payment_received`
- `unpaid`
- `unknown`

### Rules
- Never mark a bill as paid from inference alone.
- Never infer paid from zero balance alone.
- Never infer autopay from prior history alone.
- Require explicit evidence for anything stronger than `unknown`.

### Example evidence phrases
Safe evidence for stronger labels includes phrases like:
- “payment received”
- “paid on”
- “thank you for your payment”
- “AutoPay scheduled for [date]”
- “this amount will be drafted automatically”
- “do not pay, your account is enrolled in autopay”

If none of those appear clearly, use `unknown`.

## Account-number rules
- Preserve exactly what the document shows.
- If the document shows `****1234`, store that as masked, not full.
- Do not reconstruct missing digits.
- Flag likely OCR confusions like `0/O`, `1/I`, `8/B` for review on high-value docs.

## Itemization rules
For table-heavy bills:
- try to preserve each row as a structured line item
- keep description, quantity/unit if present, and amount
- if row-column structure is broken, store a lower-confidence extraction and flag it
- for high-value docs, manual cleanup is acceptable

## When multimodal vision should be used
Use direct multimodal LLM reading only as:
- fallback for ugly scans
- second pass for low-confidence OCR pages
- targeted help for broken tables, stamps, handwriting, or unusual layouts

It should not be the only system of record.

## Cost model
Rough cost target for a 10-page document set:
- OCR-first + cheap text LLM: about **$0.018**
- direct multimodal reading: about **$0.02 to $0.03**
- stronger/more conservative multimodal option: about **$0.06**

Conclusion:
OCR-first is usually the best default for cost, trust, and auditability.

## Proposed document note schema
Suggested fields:
- Title
- Document type
- Document date
- Service period
- Issuer
- Account number
- Account number type (`full`, `masked`, `unknown`)
- Workflow/archive status
- Amount due
- Amount paid
- Due date
- Payment status
- Payment evidence
- Autopay evidence
- Itemized charges
- Confidence
- Review needed
- Google Drive link
- Related topics / areas / projects
- Raw OCR

## Pilot recommendation
Start with a representative batch of 10 to 20 documents across categories:
- utility bill
- medical/insurance doc
- financial statement
- legal/official letter
- one messy table-heavy scan

For the pilot, compare:
- OCR-first pipeline quality
- fallback multimodal quality
- manual review burden
- note usefulness in Obsidian

## Bottom line
The right production design is:
- **Drive for originals**
- **OCR-first for raw text and evidence**
- **text LLM for structured extraction and note generation**
- **multimodal only as fallback**

That gives Metis a document layer that is cheap, auditable, and trustworthy enough to become real working memory.

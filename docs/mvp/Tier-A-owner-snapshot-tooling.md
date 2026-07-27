# Tier A owner-snapshot tooling

Status: Tooling ready; no owner import or Tier A gate claimed
Date: 2026-07-27
Supports: D14 fidelity preflight and E11 owner link-case selection

## Scope and safety boundary

`owner_snapshot_eval.py` inventories the current owner Obsidian vault without
following symlinks, imports its supported UTF-8 text into one disposable
simplified-core evaluation user, and verifies the resulting current manifest
path by path. The issued evaluation credential is read-only. The tool never
writes to the vault, starts a service, invokes Codex, requests an embedding, or
uses an OpenAI API key.

This is a deliberately scoped bridge, not a way to weaken D14:

- Regular supported text is preserved as exact UTF-8 bytes. The audit compares
  every relative path, byte length, and SHA-256 against the read-only evaluation
  user's manifest.
- Binary and otherwise unsupported files are still inventoried with exact byte
  length and SHA-256, but the text-only evaluation import does not publish them.
- A current filesystem snapshot has no `history=true` service-version lineage
  or deletion history. The artifact records that boundary.
- Checkpoint IDs and `parent_checkpoint_id` references that exist in checkpoint
  paths, Markdown frontmatter, or JSON are audited at source-text level.
  Evaluation import stores those files as ordinary Markdown, so it cannot prove
  checkpoint-table foreign-key resolution.

Consequently, a zero-diff supported-text audit can prepare a private E11
evaluation corpus, but it does not satisfy E11's hard Tier A precondition and
must not be reported as D14's full legacy export-with-history and binary
fidelity gate. The audit emits both verdicts separately and keeps the full D14
verdict blocked until the omitted capabilities are supplied.

## Private artifact location

Owner inventories reveal private relative paths even though they contain no
source text. Keep all live artifacts and credentials under the ignored
`operator-output/` tree, never under `eval/` or `results/`:

```bash
install -d -m 700 operator-output/owner-tier-a
```

The committed JSON schemas under `eval/` describe the artifacts:

- `owner_snapshot_inventory.schema.json`
- `owner_snapshot_audit.schema.json`
- `owner_link_candidates.schema.json`
- `owner_link_leak_request.schema.json`
- `owner_link_leak_report.schema.json`

Tests use synthetic fixtures only. No owner path, note text, question, rubric
answer, or credential is committed.

## 1. Freeze and inventory the current snapshot

Run while the vault is quiescent enough to take a stable file snapshot:

```bash
python3 owner_snapshot_eval.py inventory \
  --root /Users/aether/obsidian/notes \
  --out operator-output/owner-tier-a/inventory.json
```

Every regular file gets an exact size and SHA-256. Files no larger than 4 MiB
that have a safe workspace path, valid UTF-8, no NUL byte, and no known binary
signature or extension are the supported-text import set. All other files,
symlinks, special files, portable path collisions, and the history boundary are
explicit rows or fields rather than silent omissions. The canonical
`inventory_sha256` excludes only the observation timestamp.

The import command re-hashes every inventoried regular file before sending
anything. Any vault mutation after inventory aborts the run; regenerate the
inventory rather than mixing snapshots.

## 2. Import into an already-running disposable stack

The command does not start or alter the live Nyx stack. Point it only at an
isolated simplified-core stack with evaluation import enabled. Supply the
administrative token through the environment without printing it:

```bash
export STRAYLIGHT_API_URL=http://127.0.0.1:<isolated-port>
export STRAYLIGHT_EVAL_TOKEN='<isolated-stack admin token>'

python3 owner_snapshot_eval.py import \
  --inventory operator-output/owner-tier-a/inventory.json \
  --run-id e11-owner-snapshot-2026-07-27 \
  --case-id owner-link-corpus \
  --credential-out operator-output/owner-tier-a/read-only-credential.json \
  --out operator-output/owner-tier-a/import-audit.json
```

The credential file is created once with mode `0600`; the command refuses to
overwrite it. The emitted audit is secret-redacted. Exact and lexical indexes
must be ready immediately, while semantic may remain pending. That preserves
E11's required embeddings-pending profile and incurs no embedding spend.

Re-run the deterministic manifest audit after a restore or before a draw:

```bash
python3 owner_snapshot_eval.py verify \
  --inventory operator-output/owner-tier-a/inventory.json \
  --credential operator-output/owner-tier-a/read-only-credential.json \
  --out operator-output/owner-tier-a/verify-audit.json
```

Exit status `0` means the supported-text path/bytes/hash scope is an exact
match. Exit status `2` means a diff or preflight failure. Read
`tier_a_assessment` before assigning any broader gate status.

## 3. Select link-rich candidates without authoring answers

Generate a deterministic candidate list from notes with at least three unique,
resolved outgoing wiki links:

```bash
python3 owner_snapshot_eval.py link-candidates \
  --inventory operator-output/owner-tier-a/inventory.json \
  --min-resolved-links 3 \
  --out operator-output/owner-tier-a/link-candidates.json
```

Resolution order is source-relative exact path, vault-root exact path, then a
unique basename or stem. Ambiguous and unresolved targets are represented by
hash only. The artifact contains paths and link topology but no note excerpts,
questions, claims, or rubric answers.

The owner then chooses 8-10 candidates and authors questions whose answers
require at least two linked notes. Rubric claims are authored in a separate
pass outside the vault. Owner sign-off remains mandatory before draw 1.

## 4. Run the verbatim leak gate

Create a private request matching
`eval/owner_link_leak_request.schema.json`; do not commit it:

```json
{
  "schema": "straylight-owner-link-leak-request@v1",
  "checks": [
    {"case_id": "owner-01", "claim_id": "c1", "text": "private rubric claim"}
  ]
}
```

Then run:

```bash
python3 owner_snapshot_eval.py leak-check \
  --inventory operator-output/owner-tier-a/inventory.json \
  --request operator-output/owner-tier-a/leak-request.json \
  --out operator-output/owner-tier-a/leak-report.json
```

The comparison is a case-sensitive exact Unicode substring search over every
supported text file. Exit `0` means no claim string was found. Exit `2` means at
least one claim leaked and must be rewritten before E11. The report does not
echo claim text; it records only claim identity, claim SHA-256, and matching
paths.

## Acceptance record

Before using the corpus for E11, retain privately:

1. Inventory fingerprint and classification totals.
2. Supported-text fidelity `pass` with zero missing, unexpected, duplicate, or
   mismatched paths.
3. Source-level checkpoint-lineage result and the explicit service-resolution
   limitation.
4. Link-candidate artifact and owner sign-off on the final 8-10 cases.
5. Leak report `pass`.
6. Clean implementation commit and isolated stack image fingerprint.

Never convert the supported-text verdict into "Tier A passed." D14 remains
blocked until a `history=true` legacy export, binary byte-copy path, binary
description fidelity, and actual service checkpoint-parent resolution all
pass.

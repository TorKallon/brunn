# Owner snapshot tooling and fresh-source cutover overlay

Status: Exact fresh source overlay, replay verification, soft deletions, and re-audit passed
Date: 2026-07-31
Supports: D14 fresh-source fidelity, legacy-history overlay, and E11 owner link-case selection

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
fidelity gate. The audit emits both verdicts separately; the omitted production
capabilities were supplied and passed through the legacy runbook plus the
binary-capable overlay.

Those omitted capabilities are implemented by
[Tier-A-legacy-fidelity-runbook.md](Tier-A-legacy-fidelity-runbook.md):
current+`history=true`+delta composition, exact binary companion publication,
deterministic native-record materialization, bounded lineage replay, actual
checkpoint import, and service/export audits. Its isolated service and
round-trip gates passed. The 2026-07-31 production strategy uses that verified
history first and then an exact fresh-source overlay. The scoped
`owner_snapshot_eval.py import` remains useful for E11 selection and read-only
text checks; it is not the binary-capable production overlay command.

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

The command does not start or alter any live stack. Point it only at an
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

Never convert the supported-text verdict into a full production migration
pass. The legacy-history audit and the exact fresh binary-capable overlay each
have their own independent service/export gates.

## Annex A — exact fresh-source overlay for the direct cutover

The owner directed a fresh Markdown/binary migration when it preserves more
reasoning-relevant data or metadata. A fresh import alone is still less complete
than the verified historical composite, so the selected sequence is historical
replay first, fresh overlay second.

### Captured aggregate

The exact 2026-07-31 capture contains:

- 4,267 regular files and 298,682,825 bytes;
- 3,557 text files and 710 binary files;
- zero symlinks, special files, ignored files, or portable path collisions;
- content-independent path/size/hash ledger
  `5acc8d39a0e5bc7aad088a6488f9dd3f1c1b69c327dc53daf2c0bb8e290a4865`;
- owner-inventory fingerprint
  `b72e851714dc555d8004bf1a61b1b9a4172b71aec7c5321af638761537c441ad`;
  and
- direct import-manifest fingerprint
  `5278bbc282201d47d870666c2243547c8d7600135427cb0f57cf4a2f451dafd3`.

The snapshot was copied without following links and then inventoried from the
immutable capture. An earlier transfer attempt that rounded modification times
is invalid and must not be used as evidence.

### Delta against the verified July source

The disjoint comparison has 4,173 exact unchanged files, 12 metadata-only
changes, 21 byte changes, 61 additions, and 10 absent/moved paths. All 710
binaries are unchanged. The overlay therefore expected and observed 4,173
skips plus 94 uploads. The verification replay skipped all 4,267 and uploaded
zero.

### Production overlay record

1. **Passed:** historical replay and zero-diff audits preceded the overlay.
2. **Passed:** the captured regular files re-hashed to the three fingerprints.
3. **Passed:** unchanged binaries remained byte/receipt-identical; no binary was
   uploaded or re-described.
4. **Passed:** creates/content/metadata/skips matched the action ledger.
5. **Passed:** all ten old paths were previewed and soft-deleted; their history
   remains and replacements are active.
6. **Passed:** current-manifest replay skipped all 4,267 source files and the
   post-cutover re-audit reproduced 4,267 files, 298,682,825 bytes, and the
   unchanged fingerprint.

The expected aggregate after the source overlay and moved-path soft deletions
was 10,060 active entries and 10,120 historical versions. After agent-memory and
dormant-backup capture, the observed pre-worker service contains 13,702 active
entries and 13,831 history versions. Counts remain cross-checks, not substitutes
for the passed path/hash audits.

After the guarded backfill and final canaries, the service contains 13,709
active entries, ten deleted current paths retained in history, and 13,838
history versions. The exact source re-audit remains unchanged at 4,267 files,
298,682,825 bytes, and the recorded direct fingerprint.

### Agent-memory and dormant-backup extension

The primary exact agent-memory capture adds 398 files and 5,373,439 bytes: 329
text and 69 binary, fingerprint
`64d33e0c4263cf2344594e160933b99850c4b4c0965cb7525f6ac0a5955ec09c`.
Import and replay verification pass. The 69 binaries use deterministic archival
descriptions with zero inference API calls.

A later dormant Aether backup audit found 2,793 additional files and
93,627,020 bytes: 2,415 text and 378 binary. Source fingerprint
`8271693f85254bdf349d5536f740c4107208be17c2cba070e055ba8a948f93b1`;
wrapped fingerprint
`34697fa8408dc96a3890532436aafc9eff44c112d892f8daf8c0a42ab5102990`.
Of these, 2,386 were not byte-identical to prior captures. Import/replay passed
and the old live source was archived.

### Authority boundary

This source capture is now recovery evidence, not a second writable authority.
Codex and Aether/OpenClaw both pass D13 through their production-facing pinned
wrappers, with an unchanged post-gateway source re-audit. Neither client may
write durable memory to the old source tree or silently fall back to it when
Straylight is unavailable.

The aggregate production result is
[`results/2026-07-31-railway-simplified-cutover.md`](../../results/2026-07-31-railway-simplified-cutover.md).

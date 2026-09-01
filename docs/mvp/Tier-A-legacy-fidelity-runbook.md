# Legacy fidelity import and direct-cutover annex

Status: Railway service and full-history round-trip audits passed with zero differences
Date: 2026-07-31
Supports: D14 historical fidelity gate and direct production cutover

## What is proven now

`legacy_tier_a.py` recovers the complete owner export from the retained current
portable tree, the pinned `history=true` manifest, the exact delta, and the
native-record capture. It fails closed on checksum drift, path collisions,
missing historical bytes, an unpaired binary, a regenerated binary
description, or an unresolved checkpoint parent.

The private composite passed its local checksum/path/version/hash/size audit
with zero differences:

- 4,926 logical legacy paths and 4,955 versions, including 29 inactive
  versions;
- 710 binaries paired one-to-one with the exact legacy description bytes at
  the legacy paths;
- 5,079 structured native records retained in the exact raw archive and
  rendered with the legacy exporter's deterministic `native-record@v1`
  Markdown contract;
- one checkpoint and zero non-null parent references in this owner capture.

The corrected isolated replay also passed all of D14 gate 2. From fresh empty
volumes, all six stages imported into the exact `232e7c6` image; the service
audit matched 4,926 legacy paths, all 4,955 legacy versions, and 5,079 native
materializations with zero differences. The checkpoint imported and resumed.
The full history export contained 20,047 manifest entries, and the byte audit
matched all 10,009 current paths plus 10,038 historical versions with zero
differences.

The aggregate, content-free result remains
[`results/2026-07-27-tier-a-legacy-fidelity-preflight.json`](../../results/2026-07-27-tier-a-legacy-fidelity-preflight.json).
The original Tier A rollout is superseded by the owner's 2026-07-31 direct
Railway decision. The isolated result remains valid rehearsal evidence; Railway
now separately passes the service and round-trip audits.

The first real isolated replay found eight legacy Markdown paths where two
consecutive historical versions intentionally contain identical bytes. The
ordinary workspace write contract correctly treated the second write as a
no-op, so the service audit reported a missing lineage ordinal for each path.
That replay is evidence of a fidelity blocker, not a passing result. The
evaluation-only exact-history protocol below preserves those ordinals without
changing normal production write semantics. No same-byte binary transition was
present in the owner composite.

A subsequent diagnostic replay exposed a separate retry verifier mismatch
after the atomic binary uploads had already committed. The binary API
deliberately stores portable companion metadata in its canonical server form
instead of retaining the full legacy and replay-marker objects supplied by the
importer. The fetched manifest therefore had the correct companion path,
version, bytes, portable fields, binary pointer, and byte-copy receipt, but the
importer incorrectly required the discarded replay marker. That failure is
evidence of an importer verification bug, not a service-fidelity difference.

The corrected importer recognizes only that narrow canonical companion receipt.
It requires the exact companion path/kind/hash/size/version and portable fields,
`kind=binary_description`, the exact paired binary path and content hash,
`description_status=byte_copied`, and the exact portable-companion import format
and companion hash. A mismatch in any field fails closed. Ordinary Markdown
continues to require its full legacy target identity.

## Safety boundary

Keep every owner artifact and credential under ignored `operator-output/` with
directories mode `0700` and files mode `0600`. Never commit owner paths,
payloads, manifests, or credentials. The committed result contains aggregate
counts and content-independent fingerprints only.

The reference replay procedure below uses a fresh isolated Nyx stack with its own port, database/schema, object-store
prefix, and empty user. Build it from the exact candidate commit. Set
`BRUNN_EVALUATION_API_ENABLED=true`, because exact portable binary
companions and intentional same-byte Markdown history are deliberately
evaluation-stack-only. Stop the worker and remove OpenAI API credentials from
the API/worker environment during import. This procedure needs no reasoning
call and no embedding call, and it must not touch the live stack.

## 1. Compose and audit

Set private absolute paths for the already-captured inputs:

```bash
install -d -m 700 operator-output/tier-a-legacy

python3 legacy_tier_a.py compose \
  --current-root "$LEGACY_CURRENT_ROOT" \
  --history-manifest "$LEGACY_HISTORY_MANIFEST" \
  --delta-root "$LEGACY_DELTA_ROOT" \
  --native-records "$LEGACY_NATIVE_RECORDS" \
  --out operator-output/tier-a-legacy/composite

python3 legacy_tier_a.py audit-local \
  --root operator-output/tier-a-legacy/composite \
  --out operator-output/tier-a-legacy/local-audit.json
```

`rebuild-native` exists only for a previously verified composite created by an
older revision of this tool. It verifies the predecessor checksum tree,
hardlinks the already-pinned legacy bytes, rematerializes every native record,
and records the predecessor fingerprint:

```bash
python3 legacy_tier_a.py rebuild-native \
  --root "$VERIFIED_PREDECESSOR_COMPOSITE" \
  --out operator-output/tier-a-legacy/composite
```

Continue only if `audit-local` returns `verdict: pass` and zero differences.

## 2. Provision one empty isolated owner

Supply the isolated stack's admin token by environment; the command never
prints the returned owner token and creates the credential file once at mode
`0600`:

```bash
export BRUNN_TIER_A_ADMIN_TOKEN='<isolated-stack admin token>'

python3 legacy_tier_a.py provision-owner \
  --api-url "http://127.0.0.1:$ISOLATED_PORT" \
  --external-ref "tier-a-fidelity-$RUN_ID" \
  --credential-out operator-output/tier-a-legacy/migration-credential.json

unset BRUNN_TIER_A_ADMIN_TOKEN
```

Load the one-time migration token without echoing it:

```bash
export BRUNN_TIER_A_TOKEN="$(
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' \
  operator-output/tier-a-legacy/migration-credential.json
)"
export BRUNN_STATE_API_URL="http://127.0.0.1:$ISOLATED_PORT"
export BRUNN_STATE_API_TOKEN="$BRUNN_TIER_A_TOKEN"
```

## 3. Replay every version in bounded stages

Stage zero contains the oldest version of every legacy path. Each later stage
contains only the paths with that ordinal, so repeated imports create the exact
logical lineage without needing a special database import path. The maximum
ordinal is recorded as `max_versions_per_path`; the observed owner composite
has six stages (`0..5`). Native records are added only in the last stage.

```bash
for stage in 0 1 2 3 4; do
  python3 legacy_tier_a.py materialize-stage \
    --root operator-output/tier-a-legacy/composite \
    --stage-index "$stage" \
    --out "operator-output/tier-a-legacy/stage-$stage"

  brunn-state workspace import \
    --root "operator-output/tier-a-legacy/stage-$stage" \
    --state-dir "operator-output/tier-a-legacy/state-$stage" \
    --describe-binaries false
done

python3 legacy_tier_a.py materialize-stage \
  --root operator-output/tier-a-legacy/composite \
  --stage-index 5 \
  --include-native \
  --out operator-output/tier-a-legacy/stage-5

brunn-state workspace import \
  --root operator-output/tier-a-legacy/stage-5 \
  --state-dir operator-output/tier-a-legacy/state-5 \
  --describe-binaries false
```

Every stage has its own checksum ledger. The importer re-hashes the files before
upload, pins the expected server version, and verifies each receipt. A paired
binary and description are committed atomically; the server preserves the
description string, path, hash, size, mtime, and mode exactly and queues no
description job.

Every legacy Markdown stage entry also carries
`_brunn_tier_a_history` metadata with its target lineage ordinal and one
of two explicit semantics:

- `ordinary_content_transition`; or
- `preserve_intentional_exact_bytes_version`, emitted only when the target and
  its immediate predecessor have the same hash and size.

Before sending a write, `brunn-state` compares that target with the isolated
service's current ordinal. It uploads only from exactly `target - 1`, skips a
retry only when the service is already at the target with matching
content/portable/legacy identity, and fails closed when the service is behind,
ahead, or at the target with a different identity. A target imported by the
pre-protocol importer remains resumable only when its complete legacy metadata
already proves the same target identity. This permits an interrupted isolated
replay to be repaired without treating a merely matching hash as proof.

For the same-byte case, the API additionally verifies
`expected_version == target - 1`, identical predecessor bytes, the exact
history marker, and `BRUNN_EVALUATION_API_ENABLED=true` before inserting
the otherwise-no-op version. A retry at the exact target is a no-op only when
the stored identity matches. With the evaluation API disabled, the request is
rejected; ordinary same-byte writes keep their existing no-op behavior.

Same-byte binary history is deliberately unsupported. Stage materialization or
import must fail rather than synthesize or collapse a binary version. If a
future owner composite contains such a transition, stop and design an
equivalent object-store/version protocol before proceeding.

The same boundary applies to exact portable binary companions. A changed
companion is advanced only by the atomic binary upload; the later companion
phase verifies and skips the canonical receipt. A missing or predecessor
companion is not repaired with a standalone Markdown write, and intentional
same-byte companion history is rejected as unsupported.

## 4. Restore checkpoint semantics, then audit the service

Portable native-record Markdown preserves all structured payloads. Checkpoints
also receive their actual reserved workspace identity in topological order.
The server rejects a missing, cross-user, mismatched, or self parent before
writing the child. Each imported checkpoint is immediately resume-tested:

```bash
python3 legacy_tier_a.py import-checkpoints \
  --root operator-output/tier-a-legacy/composite \
  --api-url "$BRUNN_STATE_API_URL" \
  --out operator-output/tier-a-legacy/checkpoint-import.json

python3 legacy_tier_a.py audit-service \
  --root operator-output/tier-a-legacy/composite \
  --api-url "$BRUNN_STATE_API_URL" \
  --out operator-output/tier-a-legacy/service-audit.json
```

The owner capture has no non-null checkpoint parent, so its parent-resolution
count is zero. That is an honest, vacuous owner-corpus result—not evidence of a
multi-generation owner lineage. Synthetic unit coverage and the live
simplified-workspace contract separately prove missing-parent rejection and
valid-parent acceptance.

## 5. Download and byte-audit the full service history

The service-manifest audit proves database identities. The final export proves
the stored/downloaded bytes:

```bash
brunn-state workspace export \
  --history \
  --output operator-output/tier-a-legacy/roundtrip-export

python3 legacy_tier_a.py audit-roundtrip \
  --root operator-output/tier-a-legacy/composite \
  --export-root operator-output/tier-a-legacy/roundtrip-export \
  --out operator-output/tier-a-legacy/roundtrip-audit.json
```

Gate 2 passes only when the local, checkpoint-import, service, and round-trip
artifacts all pass with zero differences. Any unexpected path, missing
version (including an intentional same-byte ordinal), changed byte length/hash,
regenerated description, or unresolved parent is a hard failure.

## 6. Historical read-only pilot transition (superseded)

The original plan issued a `read_only` credential per client after gate 2.
The owner replaced that two-step with a direct read/write Railway cutover on
2026-07-31. Keep this section only as historical protocol context; follow D13
for the current two-client credential and canary requirements.

## Billing

The completed local preflight made zero Codex/ChatGPT inference calls, zero
embedding calls, and incurred zero API spend. The importable text upper bound
is 88,534,925 bytes. At the plan's current reference price of $0.19 per
9.6 million embedding tokens, treating every byte as a token is about $1.76;
even doubling that estimate for chunk overlap is about $3.52. Including the
later fresh/agent-memory/backup captures raises the full-cutover bound to $3.61,
still below the $20 notification threshold. Keep embeddings disabled for this fidelity run;
semantic indexing is not part of D14 gate 2.

## Annex A — 2026-07-31 direct Railway replay

This annex records only aggregate, privacy-safe facts. Owner paths, payloads,
manifests, and credentials remain in ignored private operator storage.

### Source choice

The direct cutover reuses this verified history composite first, then overlays
the exact current source snapshot. This preserves 4,955 legacy version ordinals,
5,079 native records, checkpoint material, and 710 exact binary-description
pairs while still making recent source bytes and portable metadata current.
Rebuilding only from Markdown would lose service-native history; copying only
the July service state would lose recent source changes.

### Production preconditions observed

- Railway simplified API health/readiness pass at build
  `39761166d21b0cfa44d11e3ba18a52112693d0cd`.
- All 56 migrations are applied; the simplified tables were empty before
  replay.
- Context-shaping treatments and dreaming are off; operational cache, guard,
  and timing features are on.
- A checksummed PostgreSQL dump exists and validates; S3 is external and
  versioned. The isolated restore attempt could not start because locked Nyx
  blocked Docker, so it is recorded as environment-blocked and non-blocking
  for this direct owner cutover rather than as a pass.
- Worker execution was held out of replay and no embedding/inference API call
  participated in the fidelity result.

### Recovered resumable pause

Stage zero initially wrote 598 exact entries and versions before the migration credential
hit the ordinary 600-request/minute limit. At that pause the service contained 598 queued jobs,
but held worker execution meant no embedding call occurred. The importer then
verified/skipped those exact targets and completed the remaining stages without
resetting or deleting the partial import.

The bounded repair temporarily raised the request budget, reused the same API
build, finished replay/audits, and restored 600/minute. HTTP 429 was an
operational pause, not a fidelity difference.

### Railway completion checklist

1. **Passed:** stages 0–5 completed without worker execution.
2. **Passed:** checkpoint imported and resume-tested.
3. **Passed:** `audit-service` matched 4,926 paths, 4,955 legacy versions,
   5,079 native records, and 10,038 remote history versions.
4. **Passed:** `--history` exported 20,047 copies and 797,775,263 bytes with
   manifest SHA-256
   `de37b0df888e2c1ddc6644eea8665592cd2f2c1ca7113fa001b39a04cd143941`
   and zero differences.
5. **Passed:** exact fresh overlay and ten history-preserving soft deletions.
6. **Passed:** all-skip overlay replay and unchanged source re-audit.
7. **Passed:** 600/minute limit restored; legacy/evaluation APIs off; three
   disabled routes return 404.

The fidelity checklist is complete. Both client canaries, the final web
deployment, all 12,727 backfill jobs, and the permanent one-replica worker also
pass. The operational cutover and repository publication are complete; hosted
CI stays disabled until GitHub Actions billing is repaired. See
[`results/2026-07-31-railway-simplified-cutover.md`](../../results/2026-07-31-railway-simplified-cutover.md).

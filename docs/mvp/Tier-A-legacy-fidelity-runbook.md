# Tier A legacy fidelity import

Status: Local exact-composite preflight passed; isolated service import pending
Date: 2026-07-27
Supports: D14 gate 2

## What is proven now

`legacy_tier_a.py` recovers the complete owner export from the retained current
portable tree, the pinned `history=true` manifest, the exact delta, and the
native-record capture. It fails closed on checksum drift, path collisions,
missing historical bytes, an unpaired binary, a regenerated binary
description, or an unresolved checkpoint parent.

The aggregate result is
[`results/2026-07-27-tier-a-legacy-fidelity-preflight.json`](../../results/2026-07-27-tier-a-legacy-fidelity-preflight.json).
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

This is a local preflight, not a Tier A pass. The isolated import, service
manifest audit, downloaded-byte round trip, release pin, and D13 READ canaries
remain required.

## Safety boundary

Keep every owner artifact and credential under ignored `operator-output/` with
directories mode `0700` and files mode `0600`. Never commit owner paths,
payloads, manifests, or credentials. The committed result contains aggregate
counts and content-independent fingerprints only.

Use a fresh isolated Nyx stack with its own port, database/schema, object-store
prefix, and empty user. Build it from the exact candidate commit. Set
`STRAYLIGHT_EVALUATION_API_ENABLED=true`, because exact portable binary
companions are deliberately evaluation-stack-only. Stop the worker and remove
OpenAI API credentials from the API/worker environment during import. This
procedure needs no reasoning call and no embedding call, and it must not touch
the live stack.

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
export STRAYLIGHT_TIER_A_ADMIN_TOKEN='<isolated-stack admin token>'

python3 legacy_tier_a.py provision-owner \
  --api-url "http://127.0.0.1:$ISOLATED_PORT" \
  --external-ref "tier-a-fidelity-$RUN_ID" \
  --credential-out operator-output/tier-a-legacy/migration-credential.json

unset STRAYLIGHT_TIER_A_ADMIN_TOKEN
```

Load the one-time migration token without echoing it:

```bash
export STRAYLIGHT_TIER_A_TOKEN="$(
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' \
  operator-output/tier-a-legacy/migration-credential.json
)"
export CARRYSTATE_API_URL="http://127.0.0.1:$ISOLATED_PORT"
export CARRYSTATE_API_TOKEN="$STRAYLIGHT_TIER_A_TOKEN"
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

  carrystate workspace import \
    --root "operator-output/tier-a-legacy/stage-$stage" \
    --state-dir "operator-output/tier-a-legacy/state-$stage" \
    --describe-binaries false
done

python3 legacy_tier_a.py materialize-stage \
  --root operator-output/tier-a-legacy/composite \
  --stage-index 5 \
  --include-native \
  --out operator-output/tier-a-legacy/stage-5

carrystate workspace import \
  --root operator-output/tier-a-legacy/stage-5 \
  --state-dir operator-output/tier-a-legacy/state-5 \
  --describe-binaries false
```

Every stage has its own checksum ledger. The importer re-hashes the files before
upload, pins the expected server version, and verifies each receipt. A paired
binary and description are committed atomically; the server preserves the
description string, path, hash, size, mtime, and mode exactly and queues no
description job.

## 4. Restore checkpoint semantics, then audit the service

Portable native-record Markdown preserves all structured payloads. Checkpoints
also receive their actual reserved workspace identity in topological order.
The server rejects a missing, cross-user, mismatched, or self parent before
writing the child. Each imported checkpoint is immediately resume-tested:

```bash
python3 legacy_tier_a.py import-checkpoints \
  --root operator-output/tier-a-legacy/composite \
  --api-url "$CARRYSTATE_API_URL" \
  --out operator-output/tier-a-legacy/checkpoint-import.json

python3 legacy_tier_a.py audit-service \
  --root operator-output/tier-a-legacy/composite \
  --api-url "$CARRYSTATE_API_URL" \
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
carrystate workspace export \
  --history \
  --output operator-output/tier-a-legacy/roundtrip-export

python3 legacy_tier_a.py audit-roundtrip \
  --root operator-output/tier-a-legacy/composite \
  --export-root operator-output/tier-a-legacy/roundtrip-export \
  --out operator-output/tier-a-legacy/roundtrip-audit.json
```

Gate 2 passes only when the local, checkpoint-import, service, and round-trip
artifacts all pass with zero differences. Any unexpected path, missing
version, changed byte length/hash, regenerated description, or unresolved
parent is a hard failure.

## 6. Transition to the read-only pilot

After gate 2 passes, issue one `read_only` credential per D13 client, verify
server-side write denial, and revoke the migration owner credential. Then run
all three D13 READ canary sets, including their known-answer checks. Do not
claim Tier A until release pinning and those canaries also pass.

## Billing

The completed local preflight made zero Codex/ChatGPT inference calls, zero
embedding calls, and incurred zero API spend. The importable text upper bound
is 88,534,925 bytes. At the plan's current reference price of $0.19 per
9.6 million embedding tokens, treating every byte as a token is about $1.76;
even doubling that estimate for chunk overlap is about $3.52, below the
$20 notification threshold. Keep embeddings disabled for this fidelity run;
semantic indexing is not part of D14 gate 2.

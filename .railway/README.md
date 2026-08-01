# Straylight on Railway

Status: Simplified owner deployment live; layered migration, both client
canaries, backfill, and final one-replica qualification passed. Repository
publication remains as of 2026-07-31; hosted CI stays disabled until GitHub
Actions billing is repaired.

`.railway/railway.ts` is the source of truth for the owner-alpha topology:

- one public `web` service;
- private `api`, `worker`, PostgreSQL, and Datadog Agent services;
- a persistent PostgreSQL volume in `us-west2`;
- external, private, versioned AWS S3 storage in `us-west-2`.

Railway's infrastructure-as-code SDK is beta and installed locally at an exact
version. Always invoke it through the pinned runner:

```bash
export RAILWAY_IAC_TS_BIN="$PWD/.railway/node_modules/.bin/railway-iac-ts"
railway config plan
railway config apply --yes
```

Install the pinned runner with:

```bash
npm ci --prefix .railway
```

Do not use `railway config pull` casually. It can rewrite the reviewed source
file from live state. `preserve()` entries deliberately keep secret values out
of Git; set those values with `railway variable set --stdin`.

## Deployment order

The original bootstrap and migration order is complete through both client
cutovers, final web deployment, guarded backfill, and permanent worker
qualification. Current API deployment
`6388d74a-000c-4faa-a924-16069e5b4c6c` is
successful at build
`39761166d21b0cfa44d11e3ba18a52112693d0cd`; `/health` and `/ready` pass and
56/56 migrations are applied. The request limit is 600/minute, legacy and
evaluation APIs are disabled, and all three disabled route probes return 404.

Completed:

1. Retained the checksummed PostgreSQL dump and versioned external S3 objects.
2. Replayed and zero-diff-audited all legacy history/native stages; the full
   20,047-copy export matched byte-for-byte.
3. Applied the exact 4,267-file fresh source overlay, verified an all-skip
   replay, and soft-deleted ten moved/absent paths without purging history.
4. Imported and replay-verified primary agent memory plus the newly found
   dormant Aether backup corpus; old live persistence paths are absent or
   archived.
5. Restored the ordinary API configuration and route isolation.
6. Installed distinct credentials and pinned launchers. Codex and Aether/
   OpenClaw both passed their final Straylight-only canaries.
7. Deployed and verified the final web revision. An isolated restore attempt
   could not start because locked Nyx prevented Docker access; no container
   was created. This is recorded as environment-blocked and non-blocking for
   the direct owner cutover, not as a restore pass.
8. Completed all 12,727 initial backfill jobs with zero queued, running, or
   failed. Temporary two-replica finalizer `0792432f` succeeded; permanent
   worker deployment `7af78da7-3b01-4a66-9923-3aa8184d1978` is `SUCCESS` with
   exactly one running replica and prior deployments removed.
9. Activated Railway Pro and live-resized the database volume from 5 GB to
   20 GB. The confirmed $20/month minimum is infrastructure spend, not
   embeddings. Live state and this topology both specify 20,000 MB; 13.6 GiB
   is free after backfill.

Remaining order:

1. Commit and push `main`, then publish the final verdict. Hosted CI remains
   disabled because GitHub currently rejects all jobs before execution for
   account billing/spending-limit reasons; re-enable it only after that is
   repaired so it cannot recreate failed-build email noise.

The earlier request-budget HTTP 429 was recovered idempotently without resetting
the database. No owner path, content, manifest row, or credential belongs in
committed results. See
[`results/2026-07-31-railway-simplified-cutover.md`](../results/2026-07-31-railway-simplified-cutover.md).

`straylight.rourkem.com` is the selected owner-alpha hostname. The API remains
reachable only through the web proxy, and `/api/v1/admin/*` is not exposed by
that proxy.

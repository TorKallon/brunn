# Straylight Alpha Readiness

Status: **ready to prepare a controlled owner-alpha migration as of
2026-07-27, but not ready for an unrehearsed production cutover**. The
simplified core has passed its deterministic, reasoning, continuation, and
large-corpus performance gates. The hosted owner deployment still runs the old
slow core, and its source-bearing data must be explicitly imported and verified
in the new workspace tables before traffic moves.

This is the evidence register for release-candidate qualification. A launch
record must bind all evidence to one exact clean commit. The retained candidate
does so through its generated release manifest, checksums, image archives,
standalone binaries, bundles, SBOMs, and scan reports. The register separates
work Codex can complete and verify autonomously from choices that require the
owner's approval. No owner decision may weaken the frozen read, write, capture,
dreaming, reasoning-quality, token-efficiency, isolation, or provenance
contracts.

## Current Launch Call

The simplified architecture should replace the old one:

1. The old service reproduced the production failure at 3,340 entries, returning
   HTTP 408 after 26.088 seconds before useful retrieval. The simplified service
   completed the same open/search/broad-search flow in 1.047, 0.674, and 1.867
   seconds.
2. The first strict reasoning draw put simplified Straylight 10 claims behind
   legacy and 11 behind direct Markdown across 228 claims. The targeted matched
   repeat narrowed the legacy gap to one claim across 64, and source audit found
   accepted evidence in 21 of 22 disputed simplified responses. The supported
   conclusion is no demonstrated material retrieval-driven degradation, not
   perfect parity.
3. The new database migrations are additive. They create the simplified
   workspace tables but do not copy the legacy corpus into them. Cutover
   therefore requires a lossless source-and-binary export/import with exact
   path, byte, hash, description, and checkpoint verification.
4. Automatic and manual dreaming are disabled pending owner review. Local
   request and worker gates refuse dream work while disabled. On the hosted
   legacy service, the scheduler is off, all controls are paused, no job is
   active, and the `dream` capability was removed from all three active
   credentials with their exact IDs retained in an immutable operator audit
   event for deliberate restoration.
5. Reasoning evaluations must use ChatGPT-authenticated Codex and fail closed
   when the subscription is unavailable. Usage-billed APIs are limited to
   capabilities Codex cannot provide, such as embeddings.

The remaining owner cutover sequence is: preserve an exact release commit and
images; snapshot hosted PostgreSQL and S3; rehearse the migration against a
restored copy; deploy matching API and worker images; import and verify the
owner corpus; run authenticated retrieval, write, checkpoint, export, binary,
Codex, and OpenClaw canaries; then switch traffic with the old image and backup
retained for rollback.

The 2026-07-27 Nyx migration rehearsal applied migrations 51 through 55 and
started a healthy simplified API and worker. It exposed one local checksum
drift in migration 44: the previously applied file had one additional trailing
blank line and was otherwise byte-identical. Railway and the repository shared
the same checksum; Nyx's single checksum row was repaired only after comparing
all 48 applied migrations and proving this was the sole mismatch. The legacy
indexes in migrations 49 and 50 took 135 seconds to build locally. Railway has
already applied those migrations, so its pending simplified schema change is
the additive 51-through-55 set. The new Nyx workspace tables are empty by
design; no owner corpus was silently copied.

## Autonomous Release Gates

| Gate | Required evidence | Status |
| --- | --- | --- |
| Reasoning contract | Frozen source/evaluation fingerprint and no unreviewed semantic changes | Current comparison complete; no material retrieval-driven degradation demonstrated, with RuptureOps overfetch retained as a release risk |
| Deterministic tests | Rust, Python, SPA, MCP, Compose, Caddy, and workflow contracts | Pass: 234 Rust unit and 2 integration tests; 237 Python tests with 5 explicit skips; 26 MCP tests; and 18 SPA tests |
| Live API safety | Full API smoke, read-only denial, credential boundaries, export, and deletion | Pass |
| Dependency failure safety | Database/object-store outage and recovery, proxy recovery, request limits | Pass |
| Database safety | Built-in `C.UTF-8`, page checksums, pgvector 0.8.5, fresh and no-op migrations | Pass |
| Object-store safety | Versioning, conditional create, metadata, versions, delete markers, exact purge | Local MinIO behavior pass; managed S3 qualification pending |
| Backup and restore | Checksummed coordinated backup, exact inventories, isolated restore, measured RTO | Pass for Nyx: exact full restore in 45m42s; managed S3 pending |
| Rollback | Current and saved N-1 API images ready against the current schema | Pass |
| Supply chain | Pinned bases, SBOMs, repository scan, application image scan, residual inventory | Application pass; Community MinIO restricted to temporary private Nyx use |
| Browser experience | Desktop and mobile workflow, accessibility, layout, and console checks | Pass |
| Quality and tokens | Every active main, personal, Rupture Ops, and transition card at or above flat files | Conditional pass: matched repeat is within one claim of legacy, source audit found accepted evidence in 21 of 22 disputed responses, and mean uncached input is 3.6% below direct Markdown; exact parity is not proven |
| Release identity | Clean `main` commit, immutable images/binaries, checksums, and deployment dry run | Blocked for the simplified candidate: the verified local image is explicitly labeled as a worktree build and the simplified and safety changes remain uncommitted |

## Owner Decisions

Settled choices are recorded here even when their implementation still depends
on an unsettled provider or public identity.

| Decision | Status | Recorded direction |
| --- | --- | --- |
| Public identity | Alpha identity settled | Keep Straylight for this alpha at `straylight.rourkem.com`; a permanent public identity and dedicated domain are deferred. |
| Deployment | Initial path settled | Railway is running the current owner deployment, but it remains on the old slow core until the controlled simplified migration is complete. |
| Object storage | Architecture approved; simplified migration qualification pending | Use MinIO only for local development. The hosted alpha uses temporary managed S3-compatible storage; the simplified import/export and restore path still requires rehearsal against a copy. |
| Recovery | Nyx qualified; hosted pending | The exact local database and object set restored successfully. Off-host destination, key custody, RPO/RTO policy, and managed-S3 restoration remain required before hosted or invited use. |
| Monitoring | Approved in part | Datadog metrics and structured logs are approved. Alert recipients, escalation path, and retention still need exact values. Browser RUM remains outside the alpha. |
| Source control | Created; plan-limited protections pending | Private `TorKallon/straylight` repository on GitHub with `main`, CI, Dependabot alerts, and automated security fixes. The current GitHub plan does not provide branch protection or secret scanning for a private repository; do not make it public without separate identity and launch approval. |
| Alpha cohort | Approved | Owner first, then only people the owner explicitly invites. No public signup. Every person receives a separate user account and credential. |
| Operating visibility | Approved | Begin with the owner's real usage. Keep source-use rankings visible in the Control UI and aggregate model, embedding, storage, and request consumption prominent in Datadog. Set hard spend limits only after observing the initial workload. |
| Policy and support | Pending | Final retention, privacy wording, and support expectations are not yet selected. |
| Launch | Pending | Commit and retain the simplified candidate, rehearse and verify the owner-data migration on a restored copy, deploy matching API and worker images, run Codex and OpenClaw canaries, then make an explicit go/no-go decision. |

Secrets, private keys, and bearer tokens must be supplied through approved
files or secret stores, never pasted into chat or committed to the repository.

## Candidate Record

### Retained candidate - 2026-07-25

The retained candidate is identified by the clean Git revision and tree in its
generated `manifest.json`. It has 48 applied migrations and passed:

- Rust formatting, check, Clippy, 191 unit tests, and two integration tests;
- 182 offline Python tests, with five designated live binary tests separately
  passed against disposable live read/write and read-only accounts;
- 21 MCP tests and production build;
- 18 SPA tests and production build;
- destructive live alpha safety for administrator isolation, credential
  delegation and rotation, upload-fault compensation, orphan multipart
  reconciliation, complete export, schema-derived deletion, exact object
  purge, and proof-gated account deletion;
- exact-once upload finalization, read-only exact/range/version downloads,
  expired-stage rollback, single-generation multi-batch import, and a 72 MB
  resumable upload with byte-exact round trip.

The coordinated backup is:

`runs/live-backups/20260725T062858Z-df1fddcd-f6a4-414c-8976-ed0e26645af8`

It contains a 4.2 GB PostgreSQL dump and 217 MB object-store snapshot. An
isolated full restore reproduced the database inventory and every object
version, byte-verified every database object reference, applied no new
migrations, passed operator credential-loss and compromise recovery, and
started healthy isolated API and worker processes. The original stack remained
online, and the isolated stack and volumes were removed afterward. Measured
recovery time was 2,742 seconds (45m42s).

This backup records `git_dirty: true` at `f858763`. It predates the retained
candidate and establishes data recoverability; the generated release artifact
provides the separate reproducible release identity.

### Prior autonomous candidate - 2026-07-23/24

The autonomous candidate passed:

- 77 Python tests;
- 128 Rust unit tests and one concurrency integration test, with formatting,
  Clippy, and RustSec audit gates;
- 14 SPA tests plus production build and zero npm audit findings;
- 11 MCP tests plus production build and zero npm audit findings;
- development and production Compose validation, Caddy validation, workflow
  lint, shell syntax, and repository whitespace checks;
- two full destructive API smokes, including a real GPT-5.6 deep shadow dream;
- alpha account-safety, runtime dependency-failure, object-store behavior, and
  current/N-1 rollback probes;
- desktop and mobile browser inspection with no console errors, horizontal
  overflow, clipped controls, or read-only mutation affordances.

The paired semantic harness recovered 179/180 deterministic claims in
Straylight versus 175/180 through direct files. Across 45 cases, Straylight
used 0.7% more cumulative input, 11.8% less uncached input, and 15.1% fewer
agent tool calls. The sole Straylight miss contained every required fact and
source but placed two facts in adjacent claim slots; the exact result and
targeted rerun are retained in
`results/2026-07-23-alpha-candidate-comparison.md`.

The final coordinated backup after the non-root PostgreSQL hardening is:

`/tmp/straylight-alpha-final-nonroot-tBJjCI6P/20260724T020519Z-2271caa1-a30e-43eb-be8b-4dfdbb69a24a`

It passed checksum, database inventory, database invariant, object-version,
runtime image, and Compose hash verification. Its isolated restore passed
exact inventory comparison, no-op migrations, authenticated API health, and
operator credential-loss and compromise recovery in 1,098 seconds. The
restored database ran as the unprivileged `postgres` user.

Application, SPA, MCP, PostgreSQL, Caddy, and the hardened object-store client
images scan at zero critical and zero high findings. Community MinIO still has
3 critical and 26 high findings and is not an acceptable alpha production
store. The exact release commit, immutable image digests, SBOMs, source and
dependency fingerprints, and production dry-run configuration are generated
by `scripts/fingerprint-release.sh` after the clean candidate commit. The
release is expected to remain blocked until the owner selects a qualified
deployment and the exact managed S3 target passes live qualification.

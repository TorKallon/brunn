# CarryState Alpha Readiness

Status: **no-go for owner alpha as of 2026-07-25**; core, destructive-safety,
binary-lifecycle, and local recovery qualification are green, but the live
embedding dependency and exact release identity are not

This is the evidence register for release-candidate qualification. A launch
record must bind all evidence to one exact clean commit; the current working
candidate has not reached that state. The register separates work Codex can
complete and verify autonomously from choices that require the owner's
approval. No owner decision may weaken the frozen read, write, capture,
dreaming, reasoning-quality, token-efficiency, isolation, or provenance
contracts.

## Current Launch Call

The private Nyx/Tailscale owner alpha is close, but it should not be enabled
yet:

1. A direct OpenAI embeddings request currently returns
   `429 insufficient_quota`. New semantic imports, descriptions, semantic
   retrieval, and learning therefore cannot be qualified or relied on. The
   service's `/ready` endpoint confirms configuration and local dependencies;
   it does not prove that the external OpenAI account can accept a billed
   request.
2. The current implementation is an uncommitted working tree based on
   `f858763`. It has passed the gates below, but it is not yet a clean,
   fingerprinted, reproducible release candidate.
3. Historical and screening evaluations favor the workspace approach, but a
   current matched CarryState-versus-filesystem reasoning and token run cannot
   be certified until embeddings work. The 2026-07-25 Codex/OpenClaw runtime
   smoke proves both runtimes can execute and use the filesystem tools; it is
   not publishable parity evidence because it used one repetition, had no
   matched CarryState cells, and two successfully retried parent requests did
   not produce complete token receipts.

Once working OpenAI quota is available, the remaining private-owner launch
sequence is: live semantic smoke, a matched current reasoning regression,
freeze and fingerprint the exact commit, and explicit go/no-go. A managed-S3
backup/restore drill is required before a hosted or invited-user alpha, but
not before the temporary owner-only Nyx deployment.

## Autonomous Release Gates

| Gate | Required evidence | Status |
| --- | --- | --- |
| Reasoning contract | Frozen source/evaluation fingerprint and no unreviewed semantic changes | Historical pass; current recertification blocked on embeddings |
| Deterministic tests | Rust, Python, SPA, MCP, Compose, Caddy, and workflow contracts | Pass: 193 Rust; 182 offline Python plus 5 separately live; 21 MCP; and 18 SPA tests |
| Live API safety | Full API smoke, read-only denial, credential boundaries, export, and deletion | Pass |
| Dependency failure safety | Database/object-store outage and recovery, proxy recovery, request limits | Pass |
| Database safety | Built-in `C.UTF-8`, page checksums, pgvector 0.8.5, fresh and no-op migrations | Pass |
| Object-store safety | Versioning, conditional create, metadata, versions, delete markers, exact purge | Local MinIO behavior pass; managed S3 qualification pending |
| Backup and restore | Checksummed coordinated backup, exact inventories, isolated restore, measured RTO | Pass for Nyx: exact full restore in 45m42s; managed S3 pending |
| Rollback | Current and saved N-1 API images ready against the current schema | Pass |
| Supply chain | Pinned bases, SBOMs, repository scan, application image scan, residual inventory | Application pass; Community MinIO restricted to temporary private Nyx use |
| Browser experience | Desktop and mobile workflow, accessibility, layout, and console checks | Pass |
| Quality and tokens | Every active main, personal, Rupture Ops, and transition card at or above flat files | Historical pass; current matched recertification pending |
| Release identity | Clean `main` commit, immutable images/binaries, checksums, and deployment dry run | Pending |

## Owner Decisions

Settled choices are recorded here even when their implementation still depends
on an unsettled provider or public identity.

| Decision | Status | Recorded direction |
| --- | --- | --- |
| Public identity | Product name settled | CarryState is the product name. Domain acquisition and logo approval remain, but neither blocks an owner-only Nyx alpha. |
| Deployment | Initial path settled | Start owner-only on Nyx over Tailscale; Railway is the intended first hosted evaluation. |
| Object storage | Architecture approved; exact hosted qualification pending | Use local MinIO only for the temporary private Nyx deployment. Production uses a managed, versioned S3-compatible store. |
| Recovery | Nyx qualified; hosted pending | The exact local database and object set restored successfully. Off-host destination, key custody, RPO/RTO policy, and managed-S3 restoration remain required before hosted or invited use. |
| Monitoring | Approved in part | Datadog metrics and structured logs are approved. Alert recipients, escalation path, and retention still need exact values. Browser RUM remains outside the alpha. |
| Source control | Created; plan-limited protections pending | Private `TorKallon/straylight` repository on GitHub with `main`, CI, Dependabot alerts, and automated security fixes. The current GitHub plan does not provide branch protection or secret scanning for a private repository; do not make it public without separate identity and launch approval. |
| Alpha cohort | Approved | Owner first, then only people the owner explicitly invites. No public signup. Every person receives a separate user account and credential. |
| Operating visibility | Approved | Begin with the owner's real usage. Keep source-use rankings visible in the Control UI and aggregate model, embedding, storage, and request consumption prominent in Datadog. Set hard spend limits only after observing the initial workload. |
| Policy and support | Pending | Final retention, privacy wording, and support expectations are not yet selected. |
| Launch | Pending | Restore OpenAI quota, certify the current reasoning path, retain the exact candidate, then make an explicit go/no-go decision. |

Secrets, private keys, and bearer tokens must be supplied through approved
files or secret stores, never pasted into chat or committed to the repository.

## Candidate Record

### Current working candidate - 2026-07-25

The working candidate has 48 applied migrations and passed:

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
  expired-stage rollback, atomic multi-batch import, and a 72 MB resumable
  upload with byte-exact round trip.

The coordinated backup is:

`runs/live-backups/20260725T062858Z-df1fddcd-f6a4-414c-8976-ed0e26645af8`

It contains a 4.2 GB PostgreSQL dump and 217 MB object-store snapshot. An
isolated full restore reproduced the database inventory and every object
version, byte-verified every database object reference, applied no new
migrations, passed operator credential-loss and compromise recovery, and
started healthy isolated API and worker processes. The original stack remained
online, and the isolated stack and volumes were removed afterward. Measured
recovery time was 2,742 seconds (45m42s).

This backup records `git_dirty: true` at `f858763`. It establishes data
recoverability, not reproducible release identity.

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

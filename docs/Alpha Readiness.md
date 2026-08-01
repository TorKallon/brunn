# Straylight Alpha Readiness

Status: **direct Railway owner cutover operationally complete as of
2026-07-31; repository publication pending**. The simplified API is live and
healthy; migration, source retirement, both client canaries, web, guarded
backfill, and final one-replica worker qualification pass.

This is the evidence register for release-candidate qualification. A launch
record must bind all evidence to one exact clean commit. The historical retained
candidate did so through its generated release manifest, checksums, image
archives, standalone binaries, bundles, SBOMs, and scan reports; the current
cutover still needs its final matching release record. The register separates
work Codex can complete and verify autonomously from choices that require the
owner's approval. No owner decision may weaken the frozen read, write, capture,
dreaming, reasoning-quality, token-efficiency, isolation, or provenance
contracts.

## Current launch call

The owner selected one direct production cutover on Railway. Nyx is reserved
for operation, testing, and restore rehearsal; there is no Nyx pilot and no
read-only/read-write two-step. Completion means Codex and Aether/OpenClaw use
Straylight only for durable memory and no longer write to the vault.

1. Railway serves the simplified API at build
   `39761166d21b0cfa44d11e3ba18a52112693d0cd`. Health/readiness pass, 56/56
   migrations are applied, the request limit is 600/minute, legacy/evaluation
   APIs are off, all three disabled probes return 404, context treatments and
   dreaming are off, and operational cache/guard/timings are on.
2. The pre-cutover PostgreSQL dump is checksummed and its catalog validates;
   the object store is external versioned S3. A restore drill was attempted,
   but locked Nyx prevented Docker access and no restore container was created.
   This is an environment-blocked, non-blocking exception for the direct owner
   cutover, not a claimed pass.
3. The least-loss migration and zero-diff audits pass: 4,926 legacy paths,
   4,955 legacy versions, 5,079 native records, 10,038 remote versions, and a
   20,047-copy/797,775,263-byte round trip with zero differences.
4. The exact 4,267-file source overlay and all-skip replay pass. All ten moved/
   absent paths are soft-deleted with history retained and replacements active.
   Primary agent memory plus the newly found dormant Aether backup corpus are
   imported and replay-verified; old live paths are absent or archived.
5. Codex passes fresh open/read/write/replay/checkpoint and stale-409 canaries.
   Aether/OpenClaw passes its strict post-archive cross-read, byte-identical
   replay, checkpoint/resume, no-delivery, and no-API-key-reasoning canary from
   its healthy normal gateway. Both are configured Straylight-only.
6. Reasoning uses the ChatGPT-authenticated Codex plan and fails closed.
   API-key billing is limited to embeddings; the conservative upper-bound
   estimate is $3.61, below the $20 notification threshold. Actual embedding
   billing is unavailable; the separate confirmed $20 Railway Pro minimum is
   infrastructure spend.
7. The final web deployment passes. The temporary two-replica finalizer and
   permanent one-replica worker pass after the database volume was live-resized
   from 5 GB to 20 GB. All 12,727 initial jobs finished at zero queued, running,
   or failed; 126,536 search chunks have zero missing embeddings.
8. Fresh one-replica qualification passed 30 opens and 30 exact searches with
   zero failures. Service p95 was 31.809529 ms open and 29.295206 ms search,
   below the 120 ms and 107 ms limits. The exact source re-audit remains
   unchanged at 4,267 files, 298,682,825 bytes, and the recorded fingerprint.

The aggregate execution record is
[`results/2026-07-31-railway-simplified-cutover.md`](../results/2026-07-31-railway-simplified-cutover.md).

Historical qualification still matters. The old service reproduced HTTP 408
at 3,340 entries after 26.088 seconds; the simplified service completed the
same open/search/broad-search flow in 1.047, 0.674, and 1.867 seconds. The first
strict reasoning draw scored simplified 10 claims behind legacy and 11 behind
direct Markdown across 228 claims; a matched repeat narrowed the legacy gap,
but exact parity was not proven. The direct owner decision does not relabel
that result or any E01–E11 outcome.

## Autonomous Release Gates

| Gate | Required evidence | Status |
| --- | --- | --- |
| Reasoning contract | Frozen source/evaluation fingerprint and no unreviewed semantic changes | Current comparison complete; no material retrieval-driven degradation demonstrated, with RuptureOps overfetch retained as a release risk |
| Deterministic tests | Rust, Python, SPA, MCP, Railway, and workflow contracts | Final passes: 79 targeted tests, 28 MCP, 18 web, and 10 Railway contract tests after the 20 GB IaC change; earlier 281 Rust library tests passed with 1 ignored |
| Live API safety | Full API smoke, credential boundaries, export, and deletion | API/export/delete and both client canaries pass |
| Dependency failure safety | Database/object-store outage and recovery, proxy recovery, request limits | Historical candidate pass; current isolated restore could not start on locked Nyx and is a recorded non-blocking exception |
| Database safety | Built-in `C.UTF-8`, page checksums, pgvector 0.8.5, fresh and no-op migrations | Current 56/56 migration ledger and 20 GB live/IaC volume pass; final filesystem is 25% used with 13.6 GiB free |
| Object-store safety | Versioning, conditional create, metadata, versions, delete markers, exact purge | External versioned S3 and production import/export audit pass |
| Backup and restore | Checksummed coordinated backup, exact inventories, isolated restore, measured RTO | Pre-cutover PostgreSQL dump and S3 retention pass; isolated restore not performed because locked Nyx blocked Docker, non-blocking for this direct cutover |
| Rollback | Current and saved N-1 API images ready against the current schema | Historical candidate pass; current backup checksum/catalog pass, with the restore exception recorded |
| Supply chain | Pinned bases, SBOMs, repository scan, application image scan, residual inventory | API/web pass; permanent worker deployment `7af78da7-3b01-4a66-9923-3aa8184d1978` is `SUCCESS` at one replica and prior worker deployments are removed; MinIO excluded from production |
| Browser experience | Desktop and mobile workflow, accessibility, layout, and console checks | Historical candidate pass; final web deployment root and proxied API revision pass |
| Quality and tokens | Every active main, personal, Rupture Ops, and transition card at or above flat files | Historical conditional result: matched repeat was within one claim of legacy and accepted evidence appeared in 21 of 22 disputed responses; exact parity was not proven |
| Release identity | Clean `main` commit, immutable images/binaries, checksums, and deployment proof | API, web, and permanent worker deployments pass; expected worker image digest recorded; final cutover repository publication remains. Hosted CI is unavailable until GitHub Actions billing is repaired. |

## Owner Decisions

Settled choices are recorded here even when their implementation still depends
on an unsettled provider or public identity.

| Decision | Status | Recorded direction |
| --- | --- | --- |
| Public identity | Alpha identity settled | Keep Straylight for this alpha at `straylight.rourkem.com`; a permanent public identity and dedicated domain are deferred. |
| Deployment | Selected and operationally complete | Railway Pro is the only production target. Its confirmed $20/month minimum is infrastructure spend, not embeddings. Nyx is operator/test/restore infrastructure, not a pilot. |
| Object storage | Selected and audited | Use MinIO only for local development. Production external versioned S3 passed overlay/export fidelity. |
| Recovery | Backup passed; drill environment-blocked | Checksummed PostgreSQL dump plus retained versioned S3 are available. Locked Nyx prevented Docker access, so no restore container was created; retain the drill as future recovery evidence without blocking this direct owner cutover. |
| Monitoring | Approved in part | Datadog metrics and structured logs are approved. Alert recipients, escalation path, and retention still need exact values. Browser RUM remains outside the alpha. |
| Source control | Created; noisy automation removed | Private `TorKallon/straylight` repository on GitHub with `main`. The noisy scheduled Dependabot configuration was removed and 21 bot PRs closed. GitHub rejects every CI job before execution for account billing/spending-limit reasons, so CI remains disabled until billing is repaired rather than recreating failed-build emails. |
| Alpha cohort | Approved | Owner first, then only people the owner explicitly invites. No public signup. Every person receives a separate user account and credential. |
| Operating visibility | Approved | Begin with the owner's real usage. Keep source-use rankings visible in the Control UI and aggregate model, embedding, storage, and request consumption prominent in Datadog. Set hard spend limits only after observing the initial workload. |
| Policy and support | Deferred outside owner cutover | Final invited-user retention, privacy wording, and support expectations do not gate this owner-only cutover. |
| Launch | Operational complete; publication pending | Commit, push, and publish the locally verified final verdict. Re-enable hosted CI only after GitHub Actions billing is repaired. |

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
At the time, that candidate remained blocked on provider selection and managed
S3 qualification. The 2026-07-31 Railway/S3 decision supersedes that old
current-state conclusion without changing the candidate's historical evidence.

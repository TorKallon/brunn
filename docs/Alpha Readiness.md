# Straylight Alpha Readiness

Status: autonomous qualification complete; identity, deployment, recovery,
exact provider qualification, and launch approval remain before launch

This is the evidence register for one exact candidate commit. It separates
work Codex can complete and verify autonomously from choices that require the
owner's approval. No owner decision may weaken the frozen read, write,
capture, dreaming, reasoning-quality, token-efficiency, isolation, or
provenance contracts.

## Autonomous Release Gates

| Gate | Required evidence | Status |
| --- | --- | --- |
| Reasoning contract | Frozen source/evaluation fingerprint and no unreviewed semantic changes | Pass |
| Deterministic tests | Rust, Python, SPA, MCP, Compose, Caddy, and workflow contracts | Pass |
| Live API safety | Full API smoke, read-only denial, credential boundaries, export, and deletion | Pass |
| Dependency failure safety | Database/object-store outage and recovery, proxy recovery, request limits | Pass |
| Database safety | Built-in `C.UTF-8`, page checksums, pgvector 0.8.5, fresh and no-op migrations | Pass |
| Object-store safety | Versioning, conditional create, metadata, versions, delete markers, exact purge | Behavior pass; provider approval pending |
| Backup and restore | Checksummed coordinated backup, exact inventories, isolated restore, measured RTO | Pass |
| Rollback | Current and saved N-1 API images ready against the current schema | Pass |
| Supply chain | Pinned bases, SBOMs, repository scan, application image scan, residual inventory | Application pass; Community MinIO blocked |
| Browser experience | Desktop and mobile workflow, accessibility, layout, and console checks | Pass |
| Quality and tokens | Every active main, personal, Rupture Ops, and transition card at or above flat files | Pass |
| Release identity | Clean `main` commit, immutable images/binaries, checksums, and deployment dry run | Pending |

## Owner Decisions

Settled choices are recorded here even when their implementation still depends
on an unsettled provider or public identity.

| Decision | Status | Recorded direction |
| --- | --- | --- |
| Public identity | Pending, highest priority | Select a durable product name and short URL. A descriptor near "agent-first workspace and memory" is under discussion. Logo approval follows the name. |
| Deployment | Recommendation ready | Render is recommended for application compute and managed PostgreSQL; see `docs/Production Platform Evaluation.md`. No provider-specific deployment is approved yet. |
| Object storage | Architecture approved; exact qualification pending | Production uses a managed, versioned cloud S3 store. AWS S3 is the recommended first provider. MinIO is development and destructive-test infrastructure only. |
| Recovery | Explicitly deferred | Off-host backup destination, key custody, RPO, and RTO remain a launch blocker and will be decided after deployment selection. |
| Monitoring | Approved in part | Datadog metrics and structured logs are approved. Alert recipients, escalation path, and retention still need exact values. Browser RUM remains outside the alpha. |
| Source control | Approved | Private GitHub repository under `TorKallon` for now. Public GitHub hosting is acceptable later. |
| Alpha cohort | Approved | Owner first, then only people the owner explicitly invites. No public signup. Every person receives a separate user account and credential. |
| Operating visibility | Approved | Begin with the owner's real usage. Keep source-use rankings visible in the Control UI and aggregate model, embedding, storage, and request consumption prominent in Datadog. Set hard spend limits only after observing the initial workload. |
| Policy and support | Pending | Final retention, privacy wording, and support expectations are not yet selected. |
| Launch | Pending | Explicit go/no-go on the exact fingerprinted and provider-qualified candidate. |

Secrets, private keys, and bearer tokens must be supplied through approved
files or secret stores, never pasted into chat or committed to the repository.

## Candidate Record

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

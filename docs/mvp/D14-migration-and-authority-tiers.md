# D14 — Migration and Authority Cutover

Status: Operational direct cutover and repository publication passed
Date: 2026-07-31
Depends on: D13 (D13-client-integration-and-canaries.md)
Gated by: deterministic fidelity, client, production-safety, and restore checks in this document
Runtime flag: n/a (process document; context-shaping treatments remain off)

## Owner-directed supersession

The original D14 proposed a Nyx read-only Tier A pilot, a Tier B read/write
daily-driver phase, and a two-to-four-week Markdown-authority shadow period
before Tier C. The owner explicitly rejected that two-step on 2026-07-31:
Railway has no other active user traffic, Nyx must remain available for testing,
and Codex plus Aether/OpenClaw are to cut over directly to Brunn and stop
writing durable memory to the vault.

That decision changes rollout authority, not historical evidence. The E01–E11
results remain exactly as recorded, all rejected or unresolved feature flags
remain off, and the incomplete parity result is not relabeled a pass. This
document now defines the lossless direct cutover the owner authorized.

## Why the migration is layered

Neither source by itself is complete:

- A fresh Markdown/binary import has the newest source bytes and portable file
  metadata but not all service-native history, native records, checkpoint
  material, or historical binary-description receipts.
- The verified July history composite preserves those service-native records
  but predates recent source additions, edits, metadata changes, and moves.

The least-loss order is therefore:

1. Replay the verified history/native composite into the empty simplified
   tables.
2. Overlay an exact fresh source snapshot so current bytes and portable
   metadata win.
3. Soft-delete old logical paths absent from the fresh source while retaining
   their historical versions.
4. Capture current Codex and Aether local durable memory before disabling those
   persistence paths.
5. Capture any additional non-identical dormant backup corpus before archiving
   its old live source.

No step regenerates binary descriptions or replaces exact source with inferred
text. The source capture remains read-only throughout.

## Current production evidence (2026-07-31)

| Area | Observed state | Completion state |
| --- | --- | --- |
| API release | Deployment `6388d74a-000c-4faa-a924-16069e5b4c6c`, build `39761166d21b0cfa44d11e3ba18a52112693d0cd`; health/readiness/dependencies pass; web deployment `316d90eb-d807-4091-84d4-8ba10b49a2f2` passes; permanent worker deployment `7af78da7-3b01-4a66-9923-3aa8184d1978` is `SUCCESS` at one replica | Pass; prior worker deployments removed |
| Schema | 56 of 56 migrations applied | Pass |
| Runtime safety | 600/minute limit; legacy/evaluation APIs off; three disabled probes 404; context treatments and dreaming off; operational cache/guard/timings on | Pass |
| Backup | Checksummed 273,563,054-byte PostgreSQL dump; versioned external S3 retained; dump catalog validates; locked Nyx blocked Docker before an isolated restore container could be created | Backup passes; restore is `not_performed_environment_blocked` and non-blocking for this direct cutover |
| Historical source | Zero-diff 4,926 paths / 4,955 legacy versions / 5,079 native records / 10,038 remote versions; 20,047-copy, 797,775,263-byte round trip | Pass |
| Fresh source | Exact 4,267-file overlay and all-skip replay; ten soft deletions retain history; final re-audit unchanged at 4,267 files / 298,682,825 bytes / recorded fingerprint | Pass |
| Agent memory | 398-file primary capture plus 2,793-file dormant Aether backup; both imported and replay-verified | Pass |
| Worker/backfill | 12,727 initial jobs to zero queued/running/failed; 126,536 search chunks; zero missing embeddings; final 30-open/30-search qualification has zero failures and p95 31.809529/29.295206 ms | Pass |
| Current service | 13,709 active / 13,838 history; ten deleted current paths retained in history | Pass |
| Storage | Railway Pro; live/IaC volume 20 GB; filesystem 18.3 GiB, 25% used, 13.6 GiB free; database 4,094,842,547 bytes; HNSW indexes retained as derived accelerators; `corpus_members` retained until restore-backed legacy retirement | Pass for capacity; storage-efficiency audit recommended |
| Clients | Separate credentials/pinned wrappers; Codex and Aether/OpenClaw final passes | Pass |
| Authority | Both configured Brunn-only; old live vault/local-memory/report/backup paths absent; post-gateway source inventory unchanged | Pass |

The earlier HTTP 429 pause was recovered by idempotent replay without resetting
the database.
The authoritative aggregate record is
[`results/2026-07-31-railway-simplified-cutover.md`](../../results/2026-07-31-railway-simplified-cutover.md).

## Direct-cutover gates

### 1. Release and topology

- API, worker, and web report the intended final Git revision.
- PostgreSQL has exactly the expected 56 migrations.
- Health/readiness pass through the public web proxy and the API remains private.
- Public admin/evaluation routes return 404 after migration access is disabled.
- All experimental context-shaping flags and dreaming remain off.

### 2. Historical fidelity — passed

Replay every bounded stage, then import and resume-test checkpoints. Require
zero differences for:

- every logical path, version ordinal, byte length, and SHA-256;
- all 710 byte-copied binary-description pairs, never regenerated;
- all 5,079 native-record materializations;
- checkpoint identities and every non-null parent reference; and
- the downloaded full-history export.

The owner capture contains one checkpoint and no non-null parent, so its
parent-resolution count is honestly vacuous. Synthetic contract tests provide
separate parent acceptance/rejection coverage.

### 3. Fresh-source overlay — passed

Re-hash the exact fresh snapshot immediately before import. The observed
content-independent ledger is
`5acc8d39a0e5bc7aad088a6488f9dd3f1c1b69c327dc53daf2c0bb8e290a4865`.
Against the July capture it contained 4,173 exact unchanged files, 12
metadata-only changes, 21 byte changes, 61 additions, and 10 absent/moved paths;
all 710 binaries were unchanged. The first import skipped 4,173 and uploaded 94;
the replay skipped all 4,267. All ten absent/moved paths were soft-deleted with
history retained and replacements active.

### 4. Agent-memory preservation and client cutover — passed

- Capture current Codex and Aether local durable memory under distinct
  Brunn namespaces before disabling either old store.
- Issue a separate read/write credential per client through the approved local
  secret store; never inline tokens in configuration.
- Launch the fixed MCP distribution pinned to the deployed revision, with
  private per-client import and asset roots.
- Run the direct-cutover D13 READ and WRITE subset for Codex and
  Aether/OpenClaw; retain the broader reusable qualification items as explicit
  future work.
- Disable Codex local memories and Aether local-memory search/flush plus all
  vault-writing instructions, skills, symlinks, and automations.
- From fresh processes, prove both clients read and write durable context only
  through Brunn.

The primary 398-file memory capture and additional 2,793-file dormant Aether
backup both passed import and replay verification. Separate credentials,
private roots, fixed launchers, and Brunn-only configuration pass. Codex
passes the executed direct-cutover subset, including stale-write HTTP 409.
Aether/OpenClaw's strict
post-archive run passes through its healthy normal gateway: cross-read,
byte-identical replay without a new write, checkpoint/resume, no delivery,
no API-key reasoning, and an unchanged source re-audit all pass.

### 5. Production normalization and recovery — passed

- **Passed:** restore the ordinary request budget and turn off the evaluation
  and legacy APIs; remove wrong variable names; verify disabled routes return 404.
- **Passed:** start the worker only after fidelity audits and observe the
  guarded queue. Temporary two-replica finalization completed the backfill;
  permanent one-replica qualification passed. The final worker emitted no
  `53100`, error, fatal, or job-failure event.
- The PostgreSQL plus S3 restore attempt could not start because locked Nyx
  prevented Docker access; no container was created. Record
  `not_performed_environment_blocked`, retain it as future recovery work, and
  do not misreport it as a pass. The owner accepted it as non-blocking for this
  direct cutover.
- Keep GitHub CI disabled through publication: GitHub currently rejects every
  job before execution for account billing/spending-limit reasons. Re-enable it
  only after billing is repaired so it does not recreate failed-build emails.
- Retain the pre-cutover backup until the owner accepts the final aggregate
  report.

## Cost boundary

All reasoning, grading, and canary inference uses the owner's
ChatGPT-authenticated Codex plan and fails closed rather than falling back to an
API key. API-key billing is allowed only for embeddings. The deliberately
conservative import upper bound is $3.61, below the owner's $20 notification
threshold. Actual embedding billing is unavailable. The separate confirmed
$20/month Railway Pro minimum is infrastructure spend, not embeddings. Stop and notify
the owner before any revised embedding estimate exceeds $20.

## Historical experiment context

The direct-cutover decision does not erase the earlier caution:

- The simplified v8 640K exact+lexical soak measured open p95 59.7 ms, search
  53.1 ms, checkpoint 17.1 ms, and resume 35.2 ms.
- The 57-case strict draw scored legacy 170/228, simplified 160/228, and direct
  Markdown 171/228; later matched repeats narrowed the observed gap, but exact
  parity was not proven.
- E01's n=3 baseline did not establish the specified non-inferiority margin.
- E04, E05, and E06 were negative; E07's mechanism passed but adoption failed;
  E08 stopped at preflight; E09–E11 were prerequisite aborts.

Those findings are why the deployed build keeps context-shaping treatments off.
They are not evidence that a second writable authority is safer than an exact,
audited Brunn cutover.

## Acceptance and rollback

The operational cutover passes: the intended worker revision, zero-job queue,
zero missing embeddings, fidelity audits, client canaries, Brunn-only
persistence proof, and fresh one-replica qualification all pass. The
environment-blocked restore exception is accepted as non-blocking and remains
future recovery work. Evidence commit
`dff91a210293483d95c9ea61c7bab865b5a60f49` is published on `origin/main`;
hosted CI is separately unavailable until GitHub Actions billing is repaired.
The verdict is `production_cutover_complete`.

Rollback uses the retained checksummed PostgreSQL backup, versioned S3 objects,
and pinned pre-cutover images. The source snapshot is retained as recovery
evidence, not reopened as an ongoing writable authority. Any observed data
loss, unresolved lineage, regenerated binary description, or cross-client
credential leak stops the cutover immediately.

## References

- [D13 client integration and canaries](D13-client-integration-and-canaries.md)
- [Tier-A legacy fidelity runbook](Tier-A-legacy-fidelity-runbook.md)
- [Tier-A owner snapshot tooling](Tier-A-owner-snapshot-tooling.md)
- [2026-07-31 aggregate cutover record](../../results/2026-07-31-railway-simplified-cutover.md)
- [2026-07-27 historical fidelity preflight](../../results/2026-07-27-tier-a-legacy-fidelity-preflight.json)
- [2026-07-28 experiment program report](../../results/2026-07-28-experiment-program-report.md)

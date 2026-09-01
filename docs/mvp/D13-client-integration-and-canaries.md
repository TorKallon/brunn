# D13 — Client Integration and Canaries

Status: Passed for the Codex and Aether/OpenClaw direct-cutover subset
Date: 2026-07-31
Depends on: none — D14 sequences this runbook
Gated by: deterministic read/write canaries below
Runtime flag: n/a (client-side runbook)

## Current scope and evidence

The owner directed Codex and Aether/OpenClaw to use the Railway simplified
service as their only durable memory store. The prior proposal to exercise
read-only clients on Nyx first is superseded. Nyx remains the operator/test
host and must not become a second production workspace.

The deployed simplified API reports build
`39761166d21b0cfa44d11e3ba18a52112693d0cd`. A matching, immutable MCP bundle
with SHA-256
`2c7e200f2ee015cdb69ab0b0a8ad86b96391ea6573be8e4b3e2001719b8cb39c`
passes 28/28 tests. Separate credentials, fixed wrappers, private roots, and
Brunn-only configuration are installed for both clients. Their prior live
vault symlink and retired local-memory, report, and backup persistence paths are
absent; the source vault remains intact as read-only recovery evidence. The
additional dormant Aether backup was captured, imported, replay-verified, and
archived.

Codex passes fresh open/read/write/replay/checkpoint checks; a stale write
returns HTTP 409 `entry_version_conflict`. Aether/OpenClaw's strict post-archive
rerun passes cross-read, byte-identical path/ref replay without a new write,
checkpoint/resume, exact no-delivery behavior, and no-API-key reasoning. Its
normal-gateway MCP read passes; seven calls produced zero failures, fallbacks,
or outbound events, and the post-gateway source inventory remained unchanged.

Automation retirement also passes. OpenClaw has 22 jobs: three safe jobs are
enabled and 19 old vault/local-memory jobs are disabled. Codex has five active
automations, including the two rewritten for Brunn; its legacy Gmail
automation is paused. Absolute-retired-path scans pass for every active job.

All 2026-07-24 MCP evaluation results exercised the legacy core. They remain
historical interface evidence but do not satisfy these canaries.

## Fixed launcher contract

Each client launches the same MCP distribution pinned to the deployed Git
revision, but through a separate wrapper and private roots:

- a fixed Railway API URL ending in `/api`;
- a fixed, immutable MCP bundle revision;
- one secret-store account and one read/write credential per client;
- one mode-0700 import root and asset root per client;
- a cleaned child environment with API credentials and routing overrides not
  required by the MCP server removed; and
- startup rejection for mutable bundles, symlinks, unsafe ownership, or unsafe
  permissions.

Tokens are read at launch from the approved local secret store. They are never
written into Codex configuration, OpenClaw configuration, wrappers, logs, or
repository files. Separate credentials make revocation and audit attribution
surgical.

Direct cutover uses read/write credentials immediately because the owner
rejected the read-only/read-write two-step. That makes the canaries and stale
version/idempotency checks mandatory before the cutover can be called complete.

## Codex — passed

1. Register a single `brunn` stdio MCP server that invokes only the Codex
   wrapper; do not point configuration at a mutable repository build.
2. Capture existing Codex durable local memory into Brunn before changing
   its local-memory setting.
3. Disable Codex local memories and remove every instruction that tells Codex
   to persist durable decisions to the vault.
4. Start a fresh Codex process and run the direct-cutover canary subset recorded
   below.
5. Prove a new durable write is visible through Brunn and is not created
   in the old local-memory or vault locations.

## Aether/OpenClaw — passed

1. Register the same pinned distribution through the OpenClaw wrapper using a
   distinct credential and roots.
2. Capture existing Aether daily/topic memory and its durable memory document
   into Brunn before disabling them.
3. Disable the built-in memory plugin, local memory search, compaction memory
   flush, and vault-oriented skills/instructions. Remove the live vault symlink.
4. Keep every automation that has not been rewritten for Brunn disabled.
5. Restart/reload OpenClaw, run the direct-cutover canary subset recorded below
   from a fresh process, and prove no durable write reaches local memory or the
   vault.

Claude Code was included in the original pilot design. It is not part of the
owner's current two-client request and does not block this cutover. If added
later, it receives its own wrapper, credential, roots, and complete canary run.

## Canary checklists

The direct-cutover evidence executed the checks marked **passed** below. The
remaining checks are the broader reusable client-qualification contract; they
must be run before claiming that broader contract, but they are not represented
as completed by this direct-cutover aggregate.

### READ set

1. **Passed:** `memory.open` with task and hints returns evidence and passes an independently
   known-answer check. A non-empty or non-`no_evidence` response alone is
   vacuous.
2. **Broader qualification:** `memory.query` exact+lexical finds a known narrow fact with zero unexplained
   lane failures.
3. **Passed:** `memory.read` returns bytes whose SHA-256 matches the import/export ledger.
4. **Broader qualification:** `asset.fetch` writes a binary into the client's private asset root, verifies
   its hash, and returns only path/metadata to model context.
5. **Not applicable after completed backfill:** while embeddings are pending, the semantic gap notice is explicit and the
   exact+lexical result remains usable.
6. **Broader qualification:** a current record from the fresh overlay and one preserved historical record
   are both retrievable with correct provenance.

### WRITE set

1. **Passed:** a write with a stale `expected_version` returns a conflict and changes no
   bytes.
2. **Passed:** replaying the same idempotency key returns `no_op`.
3. **Passed:** `memory.checkpoint` followed by
   `memory.open{resume_checkpoint_ref}` returns the full checkpoint text plus
   changes since the checkpoint.
4. **Broader qualification:** `memory.changes` walks generations gaplessly from a recorded cursor.
5. **Broader qualification:** an advisory-lock 409 is surfaced and handled without data loss.
6. **Passed:** a client-specific durable write is visible to the other client after the
   change cursor advances.
7. **Passed:** neither the vault nor the retired local-memory tree receives a corresponding
   new file or mutation.

## Failure boundaries

- A known-answer miss, byte/hash mismatch, missing history, credential crossing
  client boundaries, or unexpected old-store mutation blocks completion.
- Semantic indexing delay alone does not block exact+lexical use.
- A client can be rolled back independently by deregistering its MCP server and
  revoking only its credential.
- Do not fall back to an OpenAI API key for reasoning if the ChatGPT-authenticated
  Codex plan is unavailable.

## Acceptance record

The direct owner cutover passes only when both clients have:

- immutable bundle identity recorded;
- distinct read/write credentials held outside configuration;
- the executed direct-cutover READ and WRITE subset above passing;
- cross-client visibility passing; and
- fresh-process proof that durable reads/writes use Brunn only.

Credential, launcher, configuration, cross-client, source-retirement, Codex,
and Aether/OpenClaw rows are populated. Both clients pass the direct-cutover
subset after the dormant backup archival with the ordinary Aether gateway live.
The broader reusable qualification items remain explicitly unclaimed.

## References

- [D14 migration and authority cutover](D14-migration-and-authority-tiers.md)
- [Operations](../Operations.md)
- [2026-07-31 aggregate cutover record](../../results/2026-07-31-railway-simplified-cutover.md)
- `apps/mcp` (12-tool stdio surface)

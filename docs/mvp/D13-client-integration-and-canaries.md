# D13 — Client Integration and Canaries

Status: Proposed — not started
Date: 2026-07-27
Depends on: none — D14 (D14-migration-and-authority-tiers.md) sequences this runbook and defines the tiers referenced throughout; the dependency runs D14 → D13, not the reverse
Gated by: none — canaries here are deterministic pass/fail checks, not paired-draw experiments
Runtime flag: n/a (client-side runbook; no context-shaping server change)

## Problem and evidence

No client has ever exercised the MCP server against the simplified core. Every MCP result dated 07-24 — including OpenClaw's 180/180 — was produced against the legacy core, and none of it transfers. No deployment currently holds owner data on the new core: hosted straylight.rourkem.com runs legacy at migration 50; Nyx runs the simplified schema with empty tables. The record is unambiguous that untested paths fail on first contact: the 07-10 canary failure was plausible-but-wrong retrieval that a sufficiency check alone would have blessed, and the 07-26 production collapse came from unbudgeted synchronous bookkeeping no test gated. This runbook defines how the three clients (Codex, OpenClaw, Claude Code) are wired to a simplified deployment — Nyx first — and the canary checklists each must pass, with an explicit failure budget.

The MCP surface under test: apps/mcp exposes 12 stdio tools (memory.open/query/read/changes/capture/write/checkpoint/stage/status, asset.list/metadata/fetch) targeting /v1/workspace/* exclusively, env-var auth (STRAYLIGHT_API_TOKEN required; STRAYLIGHT_API_URL defaults to the Docker-internal http://api:18110), required sandbox roots STRAYLIGHT_MCP_IMPORT_ROOT and CARRYSTATE_MCP_ASSET_ROOT, and single text-content-block JSON responses.

## Design

Order and time budget: Codex ~half day, OpenClaw ~half day, Claude Code ~1 day (nothing exists), plus a deliberate buffer of at least half a day for canary failures. The buffer is not padding; the record says untested paths fail on first contact.

### Token-minting runbook (all clients)

1. Mint via `straylight_auth.admin_issue_credential`. The credential is issued once and is unrecoverable — capture it into the env broker at the moment of issuance or plan to reissue.
2. One credential per client (codex, openclaw, claude-code). This gives per-client revocation and per-client canary attribution.
3. Issue read-only credentials for all Tier A work; read/write credentials only at Tier B entry (tiers per D14, D14-migration-and-authority-tiers.md). Read-only is capability-derived server-side (auth.rs:125-132) — a pilot client cannot corrupt the workspace even if misconfigured.
4. Storage: tokens live in the env broker and are exported into client process environments. Never write tokens into config files (Operations.md rule). The one current violation is OpenClaw, handled below.

### Codex (~half day)

Register via `--config` flags: `mcp_servers.straylight.command`, `mcp_servers.straylight.args`, `mcp_servers.straylight.env_vars`. The env_vars entry forwards variable names (STRAYLIGHT_API_URL, STRAYLIGHT_API_TOKEN, STRAYLIGHT_MCP_IMPORT_ROOT, CARRYSTATE_MCP_ASSET_ROOT); values come from the parent environment via the trusted broker, so no secret touches the config. Then re-run all canaries below — the 07-24 Codex results were legacy-core and are void here.

### OpenClaw (~half day)

The generated openclaw.json (mode 0600) currently INLINES the token, violating the Operations.md storage rule. Two acceptable paths: (a) switch generation to env brokering — a Small change; or (b) OWNER DECISION: the owner accepts the inlined token in writing, recorded in this doc's revision history. Silent acceptance is how ungated failures happen; there is no third option. Then re-run all canaries (the 180/180 result was legacy-core).

### Claude Code (~1 day; nothing exists today)

1. Build: `cd apps/mcp && npm run build`.
2. Register:
   `claude mcp add straylight -e STRAYLIGHT_API_URL=http://<host>:18110 -e STRAYLIGHT_API_TOKEN=<read-only token via broker> -e CARRYSTATE_MCP_ASSET_ROOT=/abs/asset/root -e STRAYLIGHT_MCP_IMPORT_ROOT=/abs/import/root -- node <repo>/apps/mcp/dist/index.js`
3. The default STRAYLIGHT_API_URL is Docker-internal and will not resolve from a host client — it must be overridden explicitly. Both sandbox roots must exist on disk, or memory.stage and asset.fetch hard-fail.
4. Use a READ-ONLY token first. Enforcement is server-side (auth.rs:125-132), so the pilot cannot corrupt the workspace regardless of client behavior.
5. Dotted tool-name check: the tool names (memory.open, asset.fetch, ...) are the one plausible Claude Code break point. Verify once against Claude Code's tool-name rules on first registration; if rejected, renaming is trivial and should be done once, not worked around per-call.
6. Document the memory.read caveat in the client notes: the ref-or-path requirement is a runtime `.refine` invisible to the published schema, so schema-driven clients will not discover it until a call fails.

### Canary checklists (run per client; scripted where possible, manual otherwise)

READ set — required for Tier A:

1. memory.open with task+hints returns evidence AND passes a known-answer check: a planted or independently known fact must appear in the returned context. Sufficiency != no_evidence alone is vacuous — the 07-10 failure was plausible-but-wrong retrieval that such a check would have caught.
2. memory.query (exact+lexical) finds a known narrow fact with zero lane_failures reported.
3. memory.read returns bytes whose sha256 matches the export manifest exactly.
4. asset.fetch downloads a binary, hash-verified, with bytes never entering model context (path and metadata only in the response).
5. The semantic_unavailable gap notice appears while embeddings are pending and is treated as expected, not as a failure.

WRITE set — required for Tier B:

1. memory.write with a stale expected_version returns a conflict.
2. Idempotency replay of the same write returns no_op.
3. memory.checkpoint followed by memory.open{resume_checkpoint_ref} returns the full checkpoint text plus changes_since_checkpoint.
4. memory.changes cursor walks generations gapless from a recorded starting generation.
5. An advisory-lock 409 is surfaced to the client and handled without data loss.

Every canary failure is logged with tool name, request, and raw response before any fix is attempted.

## What this does NOT change

No schema changes, no change to the /v1/workspace/* contract, no change to the 12-tool surface or the single text-content-block response shape, no client-side authorization logic (capability derivation stays server-side), no change to Markdown authority.

## Failure-mode analysis

- Vacuous canaries (07-10): countered by the mandatory known-answer check.
- Silent credential sprawl: countered by forcing the OpenClaw OWNER DECISION in writing.
- Docker-internal default URL: a predictable first-contact failure for host clients; called out so it is not misdiagnosed as an auth or server fault.
- Dotted-name rejection in Claude Code: verified once up front; rename is trivial.
- Legacy-core results treated as coverage: explicitly voided; all canaries rerun per client.
- Overfetch and paraphrase-loss risks are unaffected — this doc changes wiring, not context shaping.

## Acceptance gates

- All three clients pass the full READ set against Nyx simplified with read-only tokens (feeds gate 3 of D14).
- WRITE set passes per client before any Tier B daily-driver use.
- Zero credentials in config files, or a written owner acceptance for OpenClaw on record.
- Dotted tool names verified accepted (or renamed) in Claude Code.

## Rollout and kill switch

This is a runbook, not a flagged feature. Rollback per client is deregistration plus credential revocation; one credential per client makes revocation surgical. Read-only tokens make the entire Tier A pilot non-corrupting by construction.

## References

- D14-migration-and-authority-tiers.md (tier gating; gates 3 and this doc's canaries).
- apps/mcp (tool surface); auth.rs:125-132 (capability-derived read-only).
- Operations.md (credential storage rule).
- 07-24 MCP results (legacy-core; void for simplified), 07-10 canary retrieval failure, 07-26 bookkeeping collapse (vault notes).

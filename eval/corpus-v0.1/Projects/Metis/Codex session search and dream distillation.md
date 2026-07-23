Created: 2026-06-03
Updated: 2026-06-03
Status: Implemented first pass

Related: [[Projects/Metis/Metis|Metis]], [[Projects/Metis/Vault semantic search|Vault semantic search]], [[Projects/Aether/Aether|Aether]], [[Projects/Aether/Backlog|Aether backlog]]

## Purpose

Add local chunked search over Codex session transcripts so Aether can recover recent and historical work context that has not been promoted into durable memory or Obsidian notes.

The goal is not to make every transcript part of mandatory assistant context. The goal is to make prior work searchable on demand, with enough metadata to verify exact thread, time, repo, command, error, and outcome.

## Current local corpus

As of 2026-06-03, useful sources are:

- `/Users/aether/.codex/sessions` - active rollout JSONL files
- `/Users/aether/.codex/archived_sessions` - archived rollout JSONL files
- `/Users/aether/.codex/session_index.jsonl` - thread id, name, and update timestamps
- `/Users/aether/.codex/memories/rollout_summaries` - existing distilled summaries that point back to specific rollout files

Observed rough scale on 2026-06-03:

- 195 active rollout JSONL files
- 70 archived rollout JSONL files
- 221 rows in `session_index.jsonl`

## Implemented first pass

The first implementation now lives in `/Users/Shared/projects/metis`:

- Go CLI: `/Users/Shared/projects/metis/cmd/codex-session-search`
- Binary: `/Users/aether/.local/bin/codex-session-search`
- DB: `/Users/aether/.openclaw/codex-session-search/index.sqlite`
- Docs: `/Users/Shared/projects/metis/docs/codex-session-search.md`
- Aether handoff: [[Projects/Aether/Codex session search Aether handoff - 2026-06-03|Codex session search Aether handoff - 2026-06-03]]

The refresh job uses deterministic macOS `launchd` every 15 minutes with a bounded semantic budget, not an AI automation.

## Retrieval model

This should be a Metis sibling to vault semantic search:

- standalone local SQLite index outside the synced vault
- a first-class Golang CLI for fast, less-brittle search and retrieval
- optional local embeddings using the same OpenClaw-bundled `node-llama-cpp` / embeddinggemma stack where practical, isolated behind a stable index/search contract
- hybrid search: vector similarity plus BM25 plus metadata weighting
- CLI wrapper, for example `codex-session-search`
- exact source pointers in every result: rollout path, thread id, cwd, timestamp, event type, turn id, and line/index range when possible

The raw index should answer questions like:

- "what was I doing with Warmind an hour ago?"
- "what command failed when we last checked the OpenClaw heartbeat?"
- "find the previous thread where we discussed Codex session search"
- "what did the verifier output say before we committed?"

## Noise problem

The session corpus contains a lot of low-value material:

- repeated system and developer instructions
- large tool outputs
- duplicate forked/subagent context
- planning chatter that was superseded minutes later
- transient command output that matters only when paired with the user's task

The indexer should parse and classify before embedding. It should downweight or skip repeated base instructions, preserve user requests and final answers, keep tool command metadata, and treat large outputs as structured evidence rather than undifferentiated transcript text.

## Dream distillation

Raw search is recall. Dreaming is meaning extraction.

A separate background process could periodically read new sessions and produce a small queue of distilled candidates:

- durable user preferences
- recurring failure modes and fixes
- exact commands or verification checklists worth reusing
- repo/project handoff points
- decisions that should be promoted into Obsidian
- possible additions to global `AGENTS.md`
- likely stale or superseded memories that need review

The dream output should not silently rewrite durable memory. Safer first outputs:

- daily or per-run Markdown summaries under Aether or Briefings
- a promotion queue note for review
- optional ad-hoc memory update notes only after explicit approval
- suggested patches for `/Users/aether/.codex/AGENTS.md`, not automatic broad edits

## Implementation sketch

First pass:

1. Add a Golang tool under `/Users/Shared/projects/metis`, for example `cmd/codex-session-search/main.go`.
2. Add reusable parser/search packages under `internal/codexsessions`.
3. Use SQLite with FTS5 for durable local indexing and fast lexical retrieval.
4. Add source adapters for active sessions, archived sessions, `session_index.jsonl`, and existing Codex memory summaries.
5. Add a conservative parser for `session_meta`, `turn_context`, `event_msg`, and `response_item`.
6. Build the core commands first: `status`, `index`, `search`, `read`, and `memory`.
7. Keep embeddings optional in the first version. If semantic search is added, make the Go tool the front door and call an embedding helper behind the scenes rather than making ordinary retrieval depend on a fragile runtime path.
8. Create a wrapper command and npm scripts only as convenience shims, not as the main implementation.
9. Add an automation that runs incremental indexing on a short cadence.
10. Add a short global `AGENTS.md` section telling Codex when to use the tool.

Second pass:

1. Add the dream distiller as a separate command, not part of ordinary search.
2. Start with report-only Markdown output.
3. Use explicit review before promoting anything to memory, Obsidian project notes, or global instructions.

## Golang command shape

Proposed commands:

```bash
codex-session-search status --json
codex-session-search index --json
codex-session-search search "what were we doing with Warmind an hour ago" --limit 8 --json
codex-session-search read CHUNK_ID --json
codex-session-search memory "OpenClaw heartbeat exact OK" --limit 8 --json
codex-session-search dream --since 24h --out PATH
```

The `memory` command should search the distilled Codex memory/rollout-summary layer first, then include raw-session backreferences when available. That gives agents a fast way to retrieve high-signal memory results without crawling noisy transcripts unless needed.

Suggested implementation layout:

```text
/Users/Shared/projects/metis/
  cmd/codex-session-search/main.go
  internal/codexsessions/parser.go
  internal/codexsessions/index.go
  internal/codexsessions/search.go
  internal/codexsessions/memory.go
  internal/codexsessions/dream.go
```

The Go tool should keep outputs strictly structured and machine-readable so `AGENTS.md` can safely instruct future Codex sessions to call it.

## Global AGENTS.md idea

Potential instruction once the tool exists:

> When the current task may depend on recent or historical Codex work, use `codex-session-search` before guessing. This is especially relevant when the user says "earlier", "an hour ago", "last time", "we already did this", or asks about prior commands, failures, decisions, or handoffs. Treat search results as historical evidence, not implementation truth; verify current repo and runtime state before acting.

## Open questions

- Should the index include full tool outputs, compressed tool-output summaries, or both?
- Should dreams run hourly, nightly, or only after active sessions end?
- What belongs in automatic daily summaries versus an explicit promotion queue?
- Should Metis own the code while Aether owns the operating policy?

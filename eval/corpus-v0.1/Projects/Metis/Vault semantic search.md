Updated: 2026-05-03 16:35 PDT

Related: [[Projects/Metis/Metis|Metis]], [[Projects/Metis/Navigation and retrieval model|Navigation and retrieval model]], [[INDEX|Shared knowledge index]], [[Vault rules]]

Local/private semantic search is now available for the Obsidian vault.

## Current state

- Vault indexed: `/Users/aether/obsidian/notes`
- Index DB: `/Users/aether/.openclaw/vault-search/index.sqlite`
- CLI wrapper: `vault-search`
- Implementation: `/Users/Shared/projects/metis/scripts/vault-semantic-search.mjs`
- Assistant skill: `/Users/aether/.openclaw/workspace/skills/vault-semantic-search/SKILL.md`
- Embedding model: local `embeddinggemma-300m-qat-Q8_0.gguf` through OpenClaw's bundled `node-llama-cpp`
- Search method: hybrid semantic similarity plus lexical/path/heading weighting

## Initial index

Initial full index completed on 2026-05-03:

- 2,905 Markdown files
- 12,440 chunks
- FTS available
- Index stored outside the synced vault so vector churn does not sync through Obsidian

## Operational role

Use this for local/private vault retrieval when filename search is too narrow, especially before broad manual sweeps. Keep it separate from OpenClaw memory so the full vault stays searchable without becoming mandatory assistant context.

## Commands

```bash
vault-search status --json
vault-search index --json
vault-search search "blood pressure variability" --limit 8 --json
vault-search search "Zepbound appeal context" --sync --limit 8 --json
vault-search read CHUNK_ID --json
```

## Validation

Tested successfully with:

- built-in self-test fixture
- `Ithrion CGM blood pressure summary` → found the new health summary
- `Zepbound appeal context` → found the health assessment and Blue Shield denial note
- `Rune personal bridge Todoist today p1 tasks` → found the Rune bridge plan

## Notes

The implementation intentionally uses a standalone index instead of OpenClaw's main memory DB, so broad vault content does not pollute mandatory memory recall or slow down ordinary assistant memory search.

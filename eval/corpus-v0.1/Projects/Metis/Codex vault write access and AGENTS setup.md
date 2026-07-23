Created: 2026-05-02 12:30 PDT
Updated: 2026-05-02 12:30 PDT
Related: [[Projects/Metis/Metis|Metis]], [[INDEX|Shared knowledge index]], [[Home]], [[Briefings/Morning briefing - 2026-05-02|Morning briefing - 2026-05-02]]

## Purpose
Make the personal vault usable as durable working memory for Codex without turning the vault into an unbounded prompt dump.

The desired setup has two parts:

- Codex can write explicitly requested durable notes into `/Users/rourkem/notes` without asking for filesystem approval every time.
- Projectless Codex workspaces start with an `AGENTS.md` loader that routes agents to the vault index first, then asks them to search selectively instead of reading the whole vault.

## Codex config
User-level Codex config lives at:

```text
/Users/rourkem/.codex/config.toml
```

For normal work, keep Codex in `workspace-write` mode and add the vault as an extra writable root:

```toml
sandbox_mode = "workspace-write"

[sandbox_workspace_write]
network_access = true
writable_roots = ["/Users/rourkem/notes"]
```

If `writable_roots` already exists, merge `/Users/rourkem/notes` into the existing list rather than replacing other roots.

This is distinct from project trust. The existing project trust entry:

```toml
[projects."/Users/rourkem/notes"]
trust_level = "trusted"
```

allows Codex to load trusted project-local config, rules, and instructions when the vault itself is the project. It does not, by itself, grant write access to the vault from a different workspace.

After changing the config, restart or reload Codex so the next session context includes `/Users/rourkem/notes` under writable roots.

## Projectless AGENTS.md
Projectless Codex threads should include an `AGENTS.md` like this in their generated workspace:

```markdown
## Shared vault / knowledge base

For user project context, first read the shared vault index: `/Users/rourkem/notes/INDEX.md`

Use it as a routing map to the Obsidian vault, active projects, and repo-specific docs. Do not load or summarize the whole vault by default. Read only the specific notes needed for the task.

Priority order:
1. User’s current request
2. This repo’s `AGENTS.md`
3. `/Users/rourkem/notes/INDEX.md`
4. Relevant Obsidian project notes
5. Repo docs, tests, and source code

If project context in the vault conflicts with current repo code, treat the code as implementation truth and the vault as planning/history unless the user says otherwise. If we find docs in the repo then migrate these to the shared vault in the right project folder every time we find them in the repo.
```

## Operating rule
When a session learns something durable, write it to the vault only when it is useful beyond the current chat. Good examples:

- project routing or setup decisions
- repeatable runbooks
- stable user preferences
- gift, travel, household, or planning notes that should be available later
- conclusions that would be annoying to rediscover

Avoid saving transient brainstorming, sensitive one-off details, raw command dumps, or anything that would make future retrieval noisier.

## Suggested durable-note pattern
For personal planning threads like gift research, create a small note in the relevant private or project area with:

```markdown
Created: YYYY-MM-DD HH:mm TZ
Updated: YYYY-MM-DD HH:mm TZ
Related: [[...]]

## Confirmed preferences

## Good gift lanes

## Risky or avoid

## Current shortlist

## Already bought / past gifts
```

Use the vault as the durable source of truth, then keep thread chat focused on live iteration.

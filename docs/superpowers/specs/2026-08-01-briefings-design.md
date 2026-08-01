# Straylight Briefings: Platform Design

Status: approved by owner 2026-08-01 (fresh design superseding the 2026-07-26
"Daily Surfaces" specification's briefing/alert scope)

Owner decisions recorded 2026-08-01:

- Agents research and generate; Straylight is the platform (storage, dedupe,
  topics, surface). No collectors, feeds, or clustering inside Straylight.
- Briefings become structured editions surfaced in the existing SPA.
- Item expansion is both stored detail (instant) and a durable "go deeper"
  request loop answered by a later agent run.
- A minimal interim rewire ships first so the morning cron resumes before the
  structured feature lands.

## 1. Problem

The morning-briefing cron was disabled at the 2026-07-31 cutover because its
prompt wrote to Obsidian and local memory files (`news-brief-state.json`,
`daily-trackers.md`). Beyond the outage, three structural gaps exist:

1. **No briefing surface.** Briefings are Markdown blobs; the SPA renders
   entries as raw preformatted text. No expand-in-place, no per-item actions.
2. **Dedupe is manual.** The agent re-reads prior briefing state each run and
   reasons "already sent July 26" by hand. Token-heavy, error-prone, and the
   state file lived on a retired local path.
3. **Topic configuration is scattered.** Section order, inclusion rules, and
   formatting preferences live across the cron prompt and agent memory files
   (`briefings-news-stocks.md`), invisible to the SPA and unversioned.

## 2. Why the prior design is superseded

The 2026-07-26 Daily Surfaces spec bundled briefings, real-time alerts, agent
tasks, and a secret vault; moved news collection into Straylight (feed
collectors, WebSub, story clustering, fact extraction); and assumed the
pre-cutover rich schema. The simplified cutover removed that schema, and both
mechanisms the old spec leaned on (dream scheduler, semantic lane) are off in
production. Its briefing item contract, re-alert policy, material-change
taxonomy, and the owner-selected **Daily Thread** visual direction are retained
as inputs; everything else about its architecture is replaced by this design.

Current contracts constrain and pre-authorize the feature:

- `docs/Architecture.md` line 79: briefings are **derived and rebuildable**.
- `docs/Specification.md` line 159: briefing views are **projections over
  Markdown conventions, not new canonical records**; typed record hierarchies
  are an explicit non-goal.
- `workspace_features.rs` is the established pattern for frontmatter-derived,
  per-user, generation-keyed cached intelligence.

## 3. Architecture

Agents (the 6:30 cron, the 10:00 health job, intraday alert checks, on-demand
sessions) do research, judgment, and prose. Straylight provides:

1. **Canonical Markdown conventions** for editions, topics, and expansion
   requests (ordinary entries: versioned, searchable, exportable, in the
   change feed).
2. **A derived, rebuildable story ledger** giving dedupe uniqueness and
   delivery history.
3. **Typed workspace endpoints + thin MCP tools** for publish, dedupe-check,
   topics, and item actions.
4. **A Daily Thread surface** in the existing SPA.

The LLM does judgment; the platform does bookkeeping.

## 4. Canonical conventions

All paths are ordinary public workspace entries owned by the user.

### 4.1 Editions — `Briefings/<YYYY>/<Edition> briefing - <YYYY-MM-DD>.md`

Example: `Briefings/2026/Morning briefing - 2026-08-01.md`.

Published via `POST /v1/workspace/briefings/publish` with typed JSON. The
server deterministically renders the Markdown body (owner style: bold linked
lead sentence, one to two supporting sentences, "Why this matters:", per-stock
subsections, compact metric groups) and stores the structured payload in the
entry version's `metadata` jsonb under `briefing`. Re-publishing the same
date+edition writes a new version of the same entry (intraday updates, the
10:00 health section) with a `delta` block naming added/changed/removed items.
Identical content is a NoOp per the existing content-hash rule.

Frontmatter (rendered): `kind: briefing_edition`, `date`, `edition`,
`generated_at`, `timezone`.

Metadata schema `briefing.v1` (stored, authoritative for projections):

```json
{
  "schema": "briefing.v1",
  "date": "2026-08-01",
  "edition": "morning",
  "generated_at": "2026-08-01T06:30:00-07:00",
  "summary_md": ["one bullet per 30-second line"],
  "sections": [
    {
      "topic": "ai",
      "title": "AI",
      "items": [
        {
          "id": "openai-hf-incident",
          "kind": "news",
          "headline_md": "**[Sentence headline](https://original.publisher/...)**",
          "body_md": "One to two supporting sentences.",
          "why_it_matters": "Specific consequence.",
          "detail_md": "Stored expansion: fuller brief, measurements, context.",
          "what_changed": "Present when delta=update: the material change.",
          "delta": "new",
          "story": {
            "key": "openai-hf-eval-agent-incident",
            "urls": ["https://original.publisher/..."],
            "title": "OpenAI Hugging Face evaluation incident",
            "entities": ["OpenAI", "Hugging Face"],
            "event_at": "2026-07-28"
          },
          "times": {
            "published_at": "2026-07-28T14:00:00Z",
            "event_at": "2026-07-28",
            "first_seen_at": "2026-08-01T06:12:00-07:00"
          }
        }
      ]
    }
  ],
  "omitted": [
    {
      "story_key": "kimi-k3-weights",
      "urls": ["https://..."],
      "reason": "already delivered 2026-07-28; no material delta"
    }
  ],
  "delta": { "added": ["item-id"], "changed": [], "removed": [] }
}
```

`delta` values: `new`, `update`, `corroboration`. `kind` values: `news`,
`metric`, `health`, `ops`, `digest`, `tracker`, `schedule`. Only `id`,
`headline_md`, and `story.key` (for `news`) are required; missing metadata
must not block publication.

### 4.2 Topics — `Briefings/Topics/<slug>.md`

Frontmatter is machine-read configuration; the body is human-editable prose
instructions to the agent (what today lives in the cron prompt and
`briefings-news-stocks.md`).

```yaml
kind: briefing_topic
slug: stocks
name: Stock watchlist
section_order: 70
mode: every_briefing        # every_briefing | on_material_delta | scheduled | paused | muted
editions: [morning]         # which editions may include it (e.g. health -> [health-update])
schedule: null              # e.g. "10:00 America/Los_Angeles" for the health topic
entities: []
symbols: [GOOGL, MSFT, AMZN, NVDA, LLY]
suppress_unchanged: true
freshness_hours: 48         # underlying-event gate the agent applies
```

Body example (stocks): absolute dollar changes not percentages; ≥2% move gets
a driver deep-dive; one subsection per stock; catalyst check before calling a
move market drift.

Seasonal pauses are `mode: paused` plus prose ("resume when Crystal opens").
Muting from the surface flips `mode: muted` via a targeted frontmatter update
that preserves the body.

### 4.3 Expansion requests — `Briefings/Requests/<YYYY-MM-DD> - <item-id>.md`

Created by the item "go deeper" action. Frontmatter: `kind: briefing_request`,
`status: pending | answered`, `edition_ref`, `item_id`, `topic`, optional
owner note. Agents receive pending requests in the topics snapshot; the
answering run writes the answer into the same entry below a `## Answer`
heading, flips `status: answered`, and links any fuller research entry. The
SPA shows the answer under the originating item.

### 4.4 Interaction log — `Briefings/Feedback/<YYYY-MM>.md`

Item actions (mark read, feedback verdicts `useful | not_important |
already_knew | repeated | wrong | follow_closer`) append one line per event to
a monthly log entry (server-side append; per-path advisory lock serializes).
Read/feedback state shown in the SPA is projected from the current month's
log — rebuildable, no extra table. Feedback is agent input for future
topic tuning, surfaced in the topics snapshot.

## 5. Story ledger (the one new table)

Migration `0059_briefing_story_ledger.sql`, schema `straylight`, RLS-scoped by
`user_id` like all 0051 tables. **Derived and rebuildable** from edition
metadata (`sections[].items[].story` and `omitted[]`) — it is an operational
projection in the sense of `search_chunks`, not a new canonical record kind.

```
briefing_stories
  user_id, story_key            UNIQUE(user_id, story_key)
  title, entities text[], topic
  event_at date/null, first_seen_at, last_seen_at
  last_delivered_date, last_delivered_edition_ref, last_delivered_headline
  delivery_count, suppression_count
  status: active | dormant

briefing_story_urls
  user_id, url_hash             UNIQUE(user_id, url_hash)
  story_key, url (canonicalized: redirects unresolved server-side; tracking
  params stripped; hash over normalized form)
```

Maintained transactionally inside publish. A tested `rebuild_briefing_ledger`
function reconstructs both tables from edition entries (used for backfill from
July briefings and as the rebuildability proof in tests).

### 5.1 Dedupe check

`POST /v1/workspace/briefings/dedupe-check` with up to 64 candidates
`{urls[], title, summary, event_at?, topic?, story_key?}` returns per
candidate:

- `exact`: URL-hash or story-key hits with full delivery history
  ("delivered 2026-07-26 as *<headline>*", suppressed N times).
- `near`: lexical candidates — FTS over ledger titles/entities plus the
  recent-first workspace lexical lane restricted to `Briefings/` paths.
- `verdict_hint`: `duplicate` (exact URL + delivered), `possible_update`
  (story seen, newer event/date), `unseen`.

The agent adjudicates `near` and `possible_update`; the endpoint never
auto-suppresses. Semantic embeddings are not used (lane off in production);
the schema leaves room to add an embedding lane later without contract change.

## 6. HTTP API

All routes join `workspace_ordinary` in `api.rs`, handlers in a new
`briefing_service.rs`, responses in `WorkspaceEnvelope`, auth via existing
capabilities (`read` for reads, `save` for mutations — no new capability).

| Method | Route | Purpose |
| --- | --- | --- |
| POST | `/v1/workspace/briefings/publish` | Typed publish → render + write entry + ledger update (idempotency_key, expected_version) |
| GET | `/v1/workspace/briefings` | List editions (date-descending, keyset) from manifest + metadata |
| GET | `/v1/workspace/briefings/{date}/{edition}` | Structured edition + rendered Markdown |
| POST | `/v1/workspace/briefings/dedupe-check` | Section 5.1 |
| GET | `/v1/workspace/briefings/topics` | Parsed topics snapshot + pending requests + recent feedback |
| POST | `/v1/workspace/briefings/items/action` | `read` / `feedback` / `expand` / `mute_topic` |

Publish validates bounded sizes (existing 4 MiB entry cap governs), inserts
the `workspace_changes` row, and invalidates the workspace-features cache —
the standard mutation contract. No new config flag: the surface is additive
and rides the existing API service (no `railway.ts` / contract-test change).

## 7. MCP tools

Three thin tools in `createStraylightMcpServer`, dot-named, zod-bounded,
filesystem-free, exposed on **both** local stdio and the hosted gateway (so a
claude.ai scheduled agent can run the briefing):

- `briefing.publish` → publish endpoint (added to the write-tool set)
- `briefing.dedupe` → dedupe-check (read-only; compact via reasoning-view)
- `briefing.topics` → topics snapshot (read-only)

Agents read editions with existing `memory.read` by path. Tool-surface tests
update: local 12 → 15, remote 10 → 13, plus `remote.test.ts` count and the
remote canary script.

## 8. SPA surface (Daily Thread)

New routes in `router.tsx` + `navItems`: **Briefings** (`/briefings`,
`/briefings/$date`) and **Topics** (`/topics`). Existing design system only
(brand green tokens, Page/Section primitives, StatusBadge tones, 1100/820/600
breakpoints, axe coverage). New bundled deps: `marked` + `dompurify`
(sanitized rendering; CSP already forbids external scripts — no CDN).

Edition view: date header with generated/updated line and edition switcher;
30-second summary block with progressive disclosure; sections as index rows
(kicker = topic, bold sentence headline, state chip `New delta` / `Update` /
`Event`); expand-in-place shows `detail_md`, `what_changed`, source links with
timestamps; item actions: mark read, go deeper, feedback menu, mute topic.
Intraday revisions render as one living day with an update rail (entry version
history supplies the revisions). Topics page: DataTable + editor form
(frontmatter fields + instructions body), read-only gated; pending requests
list with answers. `/` default route stays `/work` for now.

## 9. Interim rewire (ships first)

A contract document + rewritten cron prompt, no repo code required:

- Briefing note → `memory.write` to `Briefings/<YYYY>/Morning briefing - <date>.md`
  (same style as today; frontmatter `kind: briefing_edition`).
- Dedupe state → `Briefings/State/news-brief-state.md` (fenced JSON block, the
  existing state schema, replacing the retired local file).
- Trackers → `Briefings/State/daily-trackers.md`.
- Fail closed: if Straylight is unreachable, report and stop; never write a
  local fallback.
- Idempotency keys on all writes; the 10:00 health job updates the same
  edition entry.

The structured feature then upgrades the same paths; ledger backfill imports
July editions and the interim state file. The cron runs on Aether's host —
the contract and prompt are the deliverable; wiring and re-enabling the cron
(and the catch-up briefing) happen there.

## 10. Out of scope

Collectors/feeds/WebSub in Straylight; story clustering and fact extraction as
product code; tasks; secrets; PWA/Web Push (alerts are intraday editions
delivered via the agent's existing iMessage channel); semantic-embedding
dedupe; cross-user sharing; a new capability; changes to accepted read/write/
checkpoint/dreaming semantics.

## 11. Testing and acceptance

- **Rust**: render determinism (same payload → identical Markdown/hash);
  publish idempotency (replay NoOp, conflicting content 409); revision deltas;
  ledger update + `rebuild_briefing_ledger` equivalence property (publish N
  editions, rebuild, tables identical); dedupe-check verdicts (exact URL,
  story-key update, unseen, canonicalization); topics snapshot parsing
  (including malformed frontmatter tolerance); item actions (log append,
  request creation, mute frontmatter patch preserving body).
- **MCP**: three new tool tests + updated surface lists; error passthrough.
- **Web**: vitest route tests via `createTestRouter`, axe accessibility on
  both new pages, markdown sanitization test (script/event-handler stripping).
- **Live smoke**: publish → read → dedupe-check → action cycle inside a
  provisioned evaluation user (extends `live_simple_workspace_contract.py`
  pattern).
- Gates: all existing suites stay green (`python3 -m unittest discover -s
  tests`, `apps/web` and `apps/mcp` build+test); no unsupported-claim /
  stale-as-new golden corpus in v1 (that evaluation burden stays with the
  agent's research prompt; the platform's guarantee is exact bookkeeping).

## 12. Build sequence

1. Interim rewire contract + cron prompt (document, applied outside repo).
2. Migration 0059 + `briefing_service.rs`: publish, render, ledger,
   dedupe-check, topics snapshot, item actions + Rust tests.
3. MCP tools + tests + canary update.
4. SPA: markdown renderer, Briefings pages, Topics page + tests.
5. Backfill: import existing `sources/Briefings/` editions and interim state
   into the ledger; switch the cron prompt from interim writes to
   `briefing.publish`/`briefing.dedupe`/`briefing.topics`.
6. Record decisions and checkpoint in Straylight.

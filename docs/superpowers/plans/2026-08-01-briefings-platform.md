# Briefings Platform Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Brunn's briefings platform per `docs/superpowers/specs/2026-08-01-briefings-design.md`: typed edition publishing rendered to canonical Markdown, a rebuildable story ledger with a dedupe-check endpoint, topics-as-entries with a parsed snapshot, item actions, three MCP tools, and a Daily Thread SPA surface.

**Architecture:** Agents research and publish; Brunn stores, dedupes, and displays. Editions are ordinary Markdown entries with a `briefing.v1` payload in version metadata. One derived table pair (`briefing_stories`/`briefing_story_urls`) is rebuilt from edition metadata. All routes join the existing `workspace_ordinary` surface; MCP tools are thin adapters; the SPA extends the existing design system.

**Tech Stack:** Rust (axum, sqlx-style via existing db helpers), Postgres migration 0059, TypeScript MCP (zod v4, @modelcontextprotocol/sdk), React 19 SPA (TanStack router/query, marked + dompurify).

**Verification gates (run after every task's commit):**
- Rust: `cargo test -p brunn-api` (workspace root: `cd apps/api && cargo test`) — use whatever test invocation `AGENTS.md`/existing CI-less convention uses; check `Makefile` targets first.
- Python harness: `python3 -m unittest discover -s tests -v` (must stay green; unaffected unless deployment files change — they should not).
- MCP: `cd apps/mcp && npm run build && npm test`
- Web: `cd apps/web && npm run build && npm test -- --run`

**Conventions the executor must follow (read these files before coding):**
- `apps/api/src/simple_core.rs` — handler shape (`State<AppState>` + `Extension<AuthContext>`), `auth.require(Capability::X)?` first, `state.begin_read()/begin_write()`, `WorkspaceEnvelope`, `validate_path`/`validate_public_path`/`validate_idempotency_key`, write pipeline (`prepare_markdown`/`commit_markdown`/`upsert_markdown_in_tx`), per-path advisory lock, `workspace_changes` insert returning `generation`, `state.workspace_features.invalidate(user_id)` after commit.
- `apps/api/src/workspace_features.rs` — frontmatter parsing (`parse_frontmatter`) and the snapshot-cache pattern.
- `apps/api/src/error.rs` — `ApiError::invalid/capability/not_found/conflict` with stable codes.
- `apps/api/migrations/0051_simple_workspace_core.sql:330-400` — RLS enable/force + `simple_user_select`/`simple_user_write` policies + grants to `app_rw`/`app_ro`.
- `apps/mcp/src/index.ts` — `registerJsonTool`, write-tool Set in `registerJsonToolOnServer`, surface gating.
- `apps/web/src/pages/DreamsPage.tsx`, `CapturePage.tsx`, `ControlPage.tsx` — page/query/mutation/read-only patterns.

---

## Phase A — Interim rewire contract (no repo code)

### Task 1: Interim briefing contract document

**Files:**
- Create: `docs/superpowers/specs/2026-08-01-briefings-interim-contract.md`

- [ ] **Step 1: Write the contract document** containing: target paths (`Briefings/<YYYY>/Morning briefing - <YYYY-MM-DD>.md`, `Briefings/State/news-brief-state.md`, `Briefings/State/daily-trackers.md`), frontmatter (`kind: briefing_edition` / `kind: briefing_state`), fail-closed rule (if Brunn unreachable: report and stop; never write local fallback), idempotency-key format (`briefing-<date>-<edition>-<attempt>`), the 10:00 health job updating the same edition entry, and a complete rewritten cron prompt for the 6:30 job that uses only `memory.open/query/read/write` against the hosted connector available to the cron host. The prompt must preserve the existing research pipeline (X/Discord digests, Datadog snapshots, trackers, strict <48h gate) but replace every Obsidian/local-file step with the Brunn paths above, and dedupe against `Briefings/State/news-brief-state.md` plus the last three `Briefings/` editions via `memory.read`.
- [ ] **Step 2: Commit** — `git add docs/superpowers/specs/2026-08-01-briefings-interim-contract.md && git commit -m "docs: interim briefing rewire contract"`

## Phase B — Rust: migration, render, ledger, endpoints

### Task 2: Migration 0059 — story ledger tables

**Files:**
- Create: `apps/api/migrations/0059_briefing_story_ledger.sql`

- [ ] **Step 1: Write the migration** (exact content; RLS/grant block copies the 0051 pattern):

```sql
-- Derived, rebuildable projection of briefing.v1 edition metadata.
-- Provides dedupe uniqueness and delivery history. Canonical truth remains
-- the Briefings/ markdown entries; rebuild_briefing_ledger reconstructs
-- these tables from edition metadata at any time.

CREATE TABLE brunn.briefing_stories (
  user_id uuid NOT NULL REFERENCES brunn.users(id) ON DELETE CASCADE,
  story_key text NOT NULL CHECK (story_key ~ '^[a-z0-9][a-z0-9-]{2,79}$'),
  title text NOT NULL DEFAULT '',
  topic text NOT NULL DEFAULT '',
  entities text[] NOT NULL DEFAULT '{}',
  event_at date,
  first_seen_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  last_seen_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  last_delivered_date date,
  last_delivered_edition_ref text,
  last_delivered_headline text,
  delivery_count integer NOT NULL DEFAULT 0 CHECK (delivery_count >= 0),
  suppression_count integer NOT NULL DEFAULT 0 CHECK (suppression_count >= 0),
  status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'dormant')),
  PRIMARY KEY (user_id, story_key)
);

CREATE INDEX briefing_stories_user_seen_idx
  ON brunn.briefing_stories (user_id, last_seen_at DESC);

CREATE TABLE brunn.briefing_story_urls (
  user_id uuid NOT NULL REFERENCES brunn.users(id) ON DELETE CASCADE,
  url_hash brunn.sha256_hex NOT NULL,
  story_key text NOT NULL,
  url text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
  PRIMARY KEY (user_id, url_hash),
  FOREIGN KEY (user_id, story_key)
    REFERENCES brunn.briefing_stories(user_id, story_key)
    ON DELETE CASCADE
);

CREATE INDEX briefing_story_urls_user_story_idx
  ON brunn.briefing_story_urls (user_id, story_key);
```

then the RLS DO-block over `ARRAY['briefing_stories','briefing_story_urls']` with the same `simple_user_select` (app_rw, app_ro) and `simple_user_write` (app_rw, `has_any_capability(ARRAY['save','checkpoint','stage','dream','delete'])`) policies, and `GRANT SELECT, INSERT, UPDATE, DELETE ... TO app_rw; GRANT SELECT ... TO app_ro;` exactly as 0051:378-395 does. Verify `brunn.sha256_hex` domain exists (used by 0051 `entry_versions.content_sha256`); if it is schema-qualified differently, match it.
- [ ] **Step 2: Verify migration applies fresh + is checksum-stable** — run the repo's migration test path: check `Makefile`/`docs/Operations.md` for the local migrate command (`brunn migrate` via the api binary or compose); at minimum `cargo test` including any existing migration tests must pass, and booting the local stack (`make up` if already configured on this host) applies 0059 without error. If no local DB is available, add/extend an existing migration unit test that asserts the file parses and is registered in the embedded migration list.
- [ ] **Step 3: Commit** — `git commit -m "feat(api): briefing story ledger tables (0059)"`

### Task 3: `briefing.v1` types + deterministic edition render

**Files:**
- Create: `apps/api/src/briefing_service.rs` (types + render only in this task)
- Modify: `apps/api/src/lib.rs` (declare module)

- [ ] **Step 1: Write failing render tests** in `briefing_service.rs` `#[cfg(test)]`: a fixture `BriefingPublishRequest` with two sections (one `news` item with story/urls/why/detail/what_changed, one `metric` item minimal) asserting `render_edition_markdown(&req)` equals an exact expected string, and a second test asserting rendering the same payload twice yields byte-identical output.
- [ ] **Step 2: Define the types** (serde, all bounded):

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BriefingPublishRequest {
    pub date: String,            // YYYY-MM-DD, validated
    pub edition: String,         // ^[a-z0-9][a-z0-9-]{1,31}$ e.g. "morning"
    pub timezone: Option<String>,
    pub generated_at: Option<String>, // RFC3339
    #[serde(default)] pub summary_md: Vec<String>,      // <= 12 items
    #[serde(default)] pub sections: Vec<BriefingSection>, // <= 24
    #[serde(default)] pub omitted: Vec<BriefingOmission>, // <= 64
    pub idempotency_key: Option<String>,
    pub expected_version: Option<i64>,
}
pub struct BriefingSection { pub topic: String, pub title: String, pub items: Vec<BriefingItem> } // items <= 32
pub struct BriefingItem {
    pub id: String,              // ^[a-z0-9][a-z0-9-]{1,63}$, unique per edition
    pub kind: String,            // news|metric|health|ops|digest|tracker|schedule
    pub headline_md: String,     // <= 500 chars
    #[serde(default)] pub body_md: String,        // <= 4000
    #[serde(default)] pub why_it_matters: String, // <= 1000
    #[serde(default)] pub detail_md: String,      // <= 16000
    #[serde(default)] pub what_changed: String,   // <= 1000
    #[serde(default = "default_delta")] pub delta: String, // new|update|corroboration
    pub story: Option<BriefingStoryRef>,
    pub times: Option<BriefingTimes>,
}
pub struct BriefingStoryRef { pub key: String, #[serde(default)] pub urls: Vec<String>, // <= 8
    #[serde(default)] pub title: String, #[serde(default)] pub entities: Vec<String>, pub event_at: Option<String> }
pub struct BriefingTimes { pub published_at: Option<String>, pub event_at: Option<String>, pub first_seen_at: Option<String> }
pub struct BriefingOmission { pub story_key: Option<String>, #[serde(default)] pub urls: Vec<String>, pub reason: String }
```

- [ ] **Step 3: Implement `render_edition_markdown`** producing exactly:

```
Created: <date>
Updated: <generated_at local>

# <Edition-capitalized> briefing - <date>

Generated at <generated_at local> <TZ abbrev per timezone field, verbatim offset if unknown>.

## 30-second version

- <summary_md lines, one bullet each>

## <section.title>

<for each item:>
<headline_md paragraph — starts with the bold link the agent supplied>< >
<body_md>< >**Why this matters:** <why_it_matters>

<if what_changed non-empty:>*What changed:* <what_changed>

<if detail_md non-empty:>
<details block rendered as plain markdown under a "Details" bold label:>
**Details.** <detail_md>
```

with single blank lines between blocks, no trailing whitespace, `\n` endings, and omitted/story/times NOT rendered (metadata-only). Frontmatter is NOT used (the corpus convention is `Created:`/`Updated:` header lines, matching existing briefings); `metadata` carries structure.
- [ ] **Step 4: Run tests → green.** `cargo test briefing` in `apps/api`.
- [ ] **Step 5: Commit** — `git commit -m "feat(api): briefing.v1 types and deterministic edition render"`

### Task 4: URL canonicalization + hashing

**Files:**
- Modify: `apps/api/src/briefing_service.rs`

- [ ] **Step 1: Failing tests**: `canonicalize_url` lowercases scheme+host, strips fragment, strips `utm_*`, `fbclid`, `gclid`, `mc_cid`, `mc_eid`, `ref`, `ref_src`, `cmpid`, `smid` params, sorts remaining query params, removes default ports (`:80` http / `:443` https), preserves path case, collapses trailing slash only for empty-path roots; invalid URLs (no scheme, not http/https, > 2048 chars) return an error. `story_url_hash` = lowercase hex sha256 of the canonical string.
- [ ] **Step 2: Implement** using the `url` crate if already in `apps/api/Cargo.toml` dependencies (check; if absent, implement with a small hand parser consistent with the tests — do NOT add a dependency without checking existing ones first; `url` is almost certainly present transitively but must be a direct dep to use).
- [ ] **Step 3: Tests green. Commit** — `git commit -m "feat(api): briefing URL canonicalization and hashing"`

### Task 5: Publish endpoint + ledger update

**Files:**
- Modify: `apps/api/src/briefing_service.rs` (handler `publish`), `apps/api/src/api.rs` (route `.route("/workspace/briefings/publish", post(...))` in the `workspace_ordinary` router)

- [ ] **Step 1: Failing integration-style tests** (follow the existing simple_core test harness pattern — find how simple_core handlers are tested; if handler tests require a live DB and none exists in unit scope, write the validation-layer tests (`validate_publish_request` covering: bad date, bad edition slug, duplicate item ids, >4 MiB rendered size → `invalid_request`; delta vocabulary) plus ledger-SQL tests gated behind the repo's DB-test convention).
- [ ] **Step 2: Implement `publish`**: `auth.require(Capability::Save)?`; validate; render; entry path = `format!("Briefings/{}/{} briefing - {}.md", year, capitalized_edition, date)`; title = `format!("{} briefing - {}", capitalized_edition, date)`; write via the same pipeline `simple_core` uses (`upsert_markdown_in_tx` or equivalent public helper — if the helpers are private, add `pub(crate)` visibility rather than duplicating the pipeline), passing `metadata = {"kind":"briefing_edition","briefing": <full request echo minus idempotency/expected_version, plus computed delta block>}`; honor `expected_version` (409 `entry_version_conflict`) and content-hash NoOp; inside the same transaction upsert ledger rows: for each item with `story`: UPSERT `briefing_stories` (update title/topic/entities/event_at/last_seen_at; if `delta != "corroboration"` set last_delivered_* to this edition and increment `delivery_count`), INSERT missing `briefing_story_urls` (ON CONFLICT (user_id, url_hash) DO NOTHING); for each `omitted` with story_key: upsert story with `suppression_count + 1`, no delivery fields. Compute the `delta` block by diffing item ids against the previous version's metadata (`added/changed/removed`; `changed` = same id, different content hash of the item JSON). Insert `workspace_changes` row (the entry-write pipeline already does), invalidate workspace features. Envelope data: `{path, entry_ref, version_ref, version, content_hash, delta}`.
- [ ] **Step 3: Register route in `api.rs`; `cargo build` + tests green.**
- [ ] **Step 4: Commit** — `git commit -m "feat(api): briefing publish endpoint with ledger update"`

### Task 6: Ledger rebuild + equivalence test

**Files:**
- Modify: `apps/api/src/briefing_service.rs`

- [ ] **Step 1: Implement `rebuild_briefing_ledger(tx, user_id)`**: DELETE both tables for user; scan entries under `Briefings/` prefix with `metadata->>'kind' = 'briefing_edition'` ordered by path/version ascending, replay every version's `briefing` metadata through the same upsert routine used by publish (factor the upsert into `apply_edition_to_ledger(tx, user_id, edition_ref, &BriefingMetadata)` so publish and rebuild share it).
- [ ] **Step 2: Equivalence test** (DB-gated like other DB tests, or pure-logic test if the repo has no DB test harness: factor `apply_edition_to_ledger` so its SQL is thin and test the shared in-memory accumulation logic on a Vec of editions, asserting publish-order replay equals rebuild replay).
- [ ] **Step 3: Commit** — `git commit -m "feat(api): briefing ledger rebuild shares publish upsert path"`

### Task 7: Dedupe-check endpoint

**Files:**
- Modify: `apps/api/src/briefing_service.rs`, `apps/api/src/api.rs` (route `POST /workspace/briefings/dedupe-check`)

- [ ] **Step 1: Failing tests** for verdict logic (`classify_candidate`): exact URL-hash hit + delivered story → `duplicate` with history; story_key hit with candidate `event_at` newer than `last_delivered_date` → `possible_update`; no hits → `unseen`.
- [ ] **Step 2: Implement handler**: `auth.require(Capability::Read)?` (rate-limited like other reads); request `{candidates: Vec<DedupeCandidate>}` (≤ 64; `{urls, title, summary, event_at, topic, story_key}` with the Task 3 bounds); for each candidate: canonicalize urls → lookup `briefing_story_urls` + `briefing_stories` by hash and by story_key; `near` lane = FTS over `briefing_stories.title` (plainto_tsquery over a small per-user set — a simple `ILIKE ANY` + trigram is NOT available; use `to_tsvector('english', title)` computed inline, the table is small) plus the existing workspace lexical candidates function restricted post-hoc to paths starting `Briefings/` (limit 5); response per candidate: `{exact: [...], near: [...], verdict_hint}` where exact entries carry `{story_key, title, last_delivered_date, last_delivered_edition_ref, last_delivered_headline, delivery_count, suppression_count}`.
- [ ] **Step 3: Route + tests green. Commit** — `git commit -m "feat(api): briefing dedupe-check endpoint"`

### Task 8: Topics snapshot, list/get, item actions

**Files:**
- Modify: `apps/api/src/briefing_service.rs`, `apps/api/src/api.rs` (routes: `GET /workspace/briefings`, `GET /workspace/briefings/{date}/{edition}`, `GET /workspace/briefings/topics`, `POST /workspace/briefings/items/action`)

- [ ] **Step 1: Failing tests**: topic frontmatter parse (valid topic; missing fields default `mode=every_briefing`, `section_order=1000`; malformed YAML → topic listed with `parse_error` and raw body preserved); item-action validation (unknown action → `invalid_request`; `expand` requires `item_id` + `edition_ref`); mute patch preserves body (`patch_topic_frontmatter` test: input doc with frontmatter + body, set `mode: muted`, body byte-identical, other keys preserved in order).
- [ ] **Step 2: Implement:**
  - `list`: manifest-scan `Briefings/<year>/` entries with `metadata->>'kind'='briefing_edition'`, date-descending, keyset by path, limit ≤ 60; return `{date, edition, path, entry_ref, version, generated_at, summary_md, section_titles, item_count}` per row (from metadata; no bodies).
  - `get_edition`: resolve path from `{date}/{edition}`, return full metadata + markdown text + version list (`GET` with `?version=` optional for history).
  - `topics_snapshot`: read `Briefings/Topics/*.md` entries, parse frontmatter with the `workspace_features.rs` parser (reuse; make `pub(crate)` if needed), sort by `section_order`; include pending `Briefings/Requests/*` (frontmatter `status: pending`) and the current month's `Briefings/Feedback/<YYYY-MM>.md` tail (last 50 lines) so one call gives an agent everything.
  - `item_action`: `auth.require(Capability::Save)?`; actions: `read`/`feedback` append `- <ts> <edition_ref> <item_id> <action>[ <verdict>]` to `Briefings/Feedback/<YYYY-MM>.md` (create if missing) via the entry-write pipeline (advisory lock serializes); `expand` creates `Briefings/Requests/<date> - <item_id>.md` (`kind: briefing_request`, `status: pending`, refs; 409 `request_exists` if present and pending); `mute_topic` patches `Briefings/Topics/<slug>.md` frontmatter `mode: muted`.
- [ ] **Step 3: Routes + tests green. Commit** — `git commit -m "feat(api): briefing list/get, topics snapshot, item actions"`

## Phase C — MCP tools

### Task 9: `briefing.publish`, `briefing.dedupe`, `briefing.topics`

**Files:**
- Modify: `apps/mcp/src/index.ts` (three `registerJsonTool` calls; add `briefing.publish` to the write-tool Set in `registerJsonToolOnServer`), `apps/mcp/src/reasoning-view.ts` (compactor for `briefing.dedupe`: drop `near` bodies beyond title+date), `apps/mcp/src/mcp-protocol.test.ts` (local list 12→15 + schema assertions), `apps/mcp/src/server-profile.test.ts` (remote list 10→13), `apps/mcp/src/remote.test.ts` (`tools.tools.length === 13`), `apps/mcp/scripts/remote-canary.mjs` (call `briefing.topics` read-only)
- Create: `apps/mcp/src/briefing-tools.test.ts`

- [ ] **Step 1: Update the three surface tests first (failing).**
- [ ] **Step 2: Register tools** — zod raw shapes mirroring Task 3/7 bounds with `.describe()` anti-hallucination text ("Exact edition date YYYY-MM-DD", "story keys copied verbatim from dedupe results; never invent"); `briefing.publish` → `client.request("/v1/workspace/briefings/publish", input)`; `briefing.dedupe` → `client.request("/v1/workspace/briefings/dedupe-check", input)`; `briefing.topics` → `client.request("/v1/workspace/briefings/topics")` (GET, no body). All three on both surfaces (no `surface === "local"` gate). Annotations: publish in write Set; dedupe/topics read-only.
- [ ] **Step 3: New tool tests** in `briefing-tools.test.ts` (mocked fetch per `api-client.test.ts` pattern): correct method/path/body, error passthrough shape, publish annotations `readOnlyHint:false`.
- [ ] **Step 4: `npm run build && npm test` green. Commit** — `git commit -m "feat(mcp): briefing.publish, briefing.dedupe, briefing.topics tools"`

## Phase D — SPA surface

### Task 10: Sanitized Markdown renderer

**Files:**
- Modify: `apps/web/package.json` (add `marked`, `dompurify`, `@types/dompurify` dev)
- Create: `apps/web/src/components/MarkdownView.tsx`, `apps/web/src/test/markdown.test.tsx`

- [ ] **Step 1: Failing test**: renders `**[X](https://e)** _y_` to bold link + em; strips `<script>`, `onerror=` attributes, `javascript:` hrefs; external links get `target="_blank" rel="noreferrer noopener"`.
- [ ] **Step 2: Implement** `MarkdownView({markdown, className})`: `marked.parse` (gfm, no `headerIds`), `DOMPurify.sanitize` with an allowlist profile, post-process anchors for target/rel, `dangerouslySetInnerHTML` inside `<div className="markdown-view">`; add `.markdown-view` typography to `styles.css` using existing tokens.
- [ ] **Step 3: Tests + build green. Commit** — `git commit -m "feat(web): sanitized markdown renderer"`

### Task 11: API client + types

**Files:**
- Modify: `apps/web/src/lib/api.ts`, `apps/web/src/lib/types.ts`, `apps/web/src/lib/workspace.ts` (extend `workspaceEntryKind` for `briefing_edition`/`briefing_topic`/`briefing_request`)

- [ ] **Step 1: Add types** `BriefingListRow`, `BriefingEditionData` (metadata mirror of `briefing.v1`), `BriefingTopicsSnapshot`, `DedupeRequest/Result` (types only where the SPA consumes them), and client methods `briefingsList(limit, afterPath?)`, `briefingGet(date, edition, version?)`, `briefingItemAction(input)`, `briefingTopics()` following the `workspace*` envelope conventions.
- [ ] **Step 2: Unit test** in the existing api client test file pattern asserting paths/methods. Build green. **Commit** — `git commit -m "feat(web): briefing api client and types"`

### Task 12: Briefings pages (Daily Thread)

**Files:**
- Create: `apps/web/src/pages/BriefingsPage.tsx`, `apps/web/src/pages/BriefingEditionPage.tsx`, `apps/web/src/components/BriefingItemRow.tsx`
- Modify: `apps/web/src/router.tsx` (routes `/briefings`, `/briefings/$date` with `?edition=` search param defaulting `morning`), `apps/web/src/components/AppShell.tsx` (navItems: `Briefings`, lucide `sunrise`, above Workspace), `apps/web/src/styles.css`
- Create: `apps/web/src/test/briefings.test.tsx`

- [ ] **Step 1: Failing route tests**: `/briefings` renders list rows from mocked `briefingsList`; `/briefings/2026-08-01` renders header date, summary bullets, section groups, item rows; clicking an item row toggles `.expanded-detail` visibility (stored `detail_md` via MarkdownView); item actions call `briefingItemAction` and invalidate `['briefings']`.
- [ ] **Step 2: Implement** per Daily Thread: page heading (date, `Generated 6:30 AM · Updated 10:00 AM` from metadata + version history), previous/next day links from the list data, 30-second summary block with "N more" disclosure beyond 3 bullets, sections as `briefing-index` rows (kicker = section title, headline via MarkdownView inline, state chip from `delta` mapped `new→New`, `update→Update`, `corroboration→Seen`), expand-in-place detail (detail_md, `*What changed:*`, source links with `times` timestamps, actions row: Mark read / Go deeper / Useful / Repeated / Mute topic — mutations gated by `useReadOnly()`). Styles: new classes (`.briefing-thread`, `.briefing-index-row`, `.state-chip`, `.expanded-detail`) on existing tokens; responsive rules at 820/600px (rows stack, chips inline).
- [ ] **Step 3: Extend `src/test/accessibility.test.tsx`** with both routes (mocked data) and fix findings.
- [ ] **Step 4: Tests + build green. Commit** — `git commit -m "feat(web): briefings daily-thread surface"`

### Task 13: Topics page

**Files:**
- Create: `apps/web/src/pages/TopicsPage.tsx`, `apps/web/src/test/topics.test.tsx`
- Modify: `apps/web/src/router.tsx` (`/topics`), `apps/web/src/components/AppShell.tsx` (nav `Topics`, lucide `layers-3`), `apps/web/src/styles.css`

- [ ] **Step 1: Failing tests**: renders topics table (name, mode badge, section order, editions) from mocked `briefingTopics`; editor form serializes frontmatter + body and calls `workspaceWrite` on the topic path with `expected_version`; pending requests list renders with status badges; parse_error topics render with a warning badge and raw-body editor.
- [ ] **Step 2: Implement** (ControlPage credentials-tab pattern: DataTable + inline form + `useMutation` + invalidate `['briefing-topics']`); mode select, section_order number, editions multi-input, symbols/entities chip inputs, instructions textarea (the body); read-only gating.
- [ ] **Step 3: Accessibility test for `/topics`. Tests + build green. Commit** — `git commit -m "feat(web): briefing topics management page"`

## Phase E — Verification, backfill, and cutover collateral

### Task 14: Live smoke extension

**Files:**
- Modify: `tests/live_simple_workspace_contract.py` (or create `tests/live_briefings_contract.py` following its provisioning pattern)

- [ ] **Step 1: Add a live test cycle** (evaluation-user provisioned, destructive only within it): publish a two-item edition → NoOp replay → `GET /briefings` lists it → `GET` edition returns metadata + markdown → dedupe-check with a delivered URL returns `duplicate` with history → `items/action expand` creates a pending request visible in topics snapshot → republish with one changed item returns delta `{changed:[...]}`.
- [ ] **Step 2: Run against the local stack** (`make up`), record output honestly (repo convention). **Commit** — `git commit -m "test: live briefings contract cycle"`

### Task 15: Structured cron prompt + Brunn records

**Files:**
- Create: `docs/superpowers/specs/2026-08-01-briefings-cron-prompt.md` (the post-feature prompt: topics via `briefing.topics`, dedupe via `briefing.dedupe`, publish via `briefing.publish`; the 10:00 health job republishing the same edition; alert checks as intraday republish; fail closed)

- [ ] **Step 1: Write the document.** **Step 2: Commit.**
- [ ] **Step 3: Write Brunn decision/checkpoint records** (memory.write + memory.checkpoint) summarizing what shipped, with entry refs and commit hashes.

## Self-review results

- Spec coverage: §4 conventions → Tasks 3, 5, 8; §5 ledger + rebuild + dedupe → Tasks 2, 5, 6, 7; §6 API → Tasks 5, 7, 8; §7 MCP → Task 9; §8 SPA → Tasks 10–13; §9 interim → Task 1; §11 testing → per-task tests + Task 14; §12 sequence → phase order. Backfill note: ledger seeds from structured editions only; legacy July briefings are covered by the dedupe `near` lexical lane (spec §5.1) — no legacy-parse importer (YAGNI).
- Placeholder scan: none; where the repo's private helpers constrain implementation (write pipeline visibility, DB-test harness availability), the plan names the exact fallback decision rather than deferring it.
- Type consistency: `briefing.v1` field names match across Task 3 (Rust), Task 9 (zod), Task 11 (TS); route paths match across Tasks 5/7/8/9/11.

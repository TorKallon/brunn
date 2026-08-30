# Straylight Alpha Specification

Status: implementation and acceptance contract

## Objective

A fresh authorized agent must be able to:

1. Find the relevant current Markdown and binary material without reading the
   whole workspace.
2. Read exact sources and reason at least as well as it can with local files.
3. Change ordinary workspace files without rebuilding unrelated state.
4. Leave a compact checkpoint that a fresh agent can resume.
5. Observe changes since that checkpoint.
6. Use a strictly read-only credential when mutation is inappropriate.

The alpha is not successful merely because endpoints respond or tests pass.
It must meet reasoning, evidence, latency, token, fidelity, isolation, backup,
and operational gates in this document.

## Identity And Access

- One internal `user_id` owns every entry, version, change, job, and usage row.
- There is no separate tenant ID.
- One deployment may serve multiple users.
- Credentials are bearer secrets stored only as hashes.
- A credential belongs to exactly one user.
- A read-only credential cannot create a version, change, job, upload, or
  checkpoint.
- Credential issuance and revocation require `credential:manage`.
- API responses never return another user's references or aggregate counts.

Read-only capabilities:

- `open`
- `query`
- `read`
- `status`

Read/write capabilities add:

- `save`
- `checkpoint`
- `stage`
- `delete`
- `dream`

## Entry Model

An entry has:

- stable UUID
- owning user
- case-insensitively unique relative path
- title
- kind: `markdown` or `binary`
- media type
- current version number
- created and updated time
- optional deletion time

An entry version has:

- stable UUID
- entry UUID and positive sequential version
- SHA-256
- exact text or exact object-store locator, never both
- byte length
- metadata
- creating credential
- creation time

Text versions contain the exact UTF-8 string supplied by the caller. Binary
versions identify immutable object-store bytes by key and provider version ID.
An entry's current pointer and its referenced version become visible in one
short PostgreSQL commit. Portable modification time, mode, and other
transfer-only annotations on the current version may be corrected in place
when bytes are unchanged; this emits a normal workspace change but does not
create a duplicate content version.

## Workspace Generation

Every changed entry appends one `workspace_changes` row with:

- monotonic generation
- user
- entry and version
- `create`, `update`, or `delete`
- path
- SHA-256
- recorded time

Generation is a change cursor, not a whole-workspace snapshot. It does not
promise that two reads at one generation reconstruct a globally isolated
view. A consumer that needs to detect intervening edits compares exact entry
versions or reads changes after its cursor.

## Consistency Posture

Single-entry visibility means readers see either the previous version or the
new version of one entry. It does not mean ACID coordination across a
workspace, a batch, PostgreSQL and S3, or background providers.

Batch reads retain valid items when another exact path is stale. Import,
export, embedding, description, dreaming, usage, and maintenance work may make
partial progress, record what completed, and retry the remainder. They must not
hold a corpus-sized transaction, roll back unrelated successful work, or make
ordinary reads wait for repair.

For a new Markdown path, the implementation tries the insert directly under a
path-local guard. It performs the existing-entry read and row lock only after a
path collision. Updates retain their version and stale-write checks without
making every create pay for duplicate coordination.

## Search Index

Only current Markdown versions have current `search_chunks`.

Each chunk contains:

- entry and entry-version IDs
- ordinal
- path
- heading
- source text
- token estimate
- PostgreSQL FTS vector
- optional 1,536-dimension embedding

Chunks and embeddings are rebuildable. Missing embeddings produce lexical-only
availability and a queued embedding job. Search never waits for whole-workspace
reindexing. After user-visible dreaming and binary-description jobs, workers
prefer the newest ready embedding jobs so a historical import does not starve
current work. Under sustained overload, older derived jobs may wait; exact and
lexical retrieval do not.

## Markdown Conventions

Domain concepts remain ordinary Markdown. Recommended frontmatter:

```yaml
---
kind: person | event | project | task | place | resource | note
status: current domain status
people: []
start: optional ISO date or datetime
end: optional ISO date or datetime
timezone: optional IANA timezone
source: optional source locator
authority: owner | source | inferred | unknown
---
```

Fields are optional and extensible. A file remains useful without frontmatter.
The service must not infer that one domain status changes another. For example,
an event may be scheduled while attendance remains unknown, and a booking may
be confirmed while payment remains pending.

The SPA may build people, event, task, project, logistics, briefing, and recent
change views from these conventions. Those views are projections, not new
canonical records.

## HTTP API

All `/v1/workspace` endpoints require a bearer credential except health
endpoints. JSON responses use a compact envelope containing optional session
and generation references, status, data, and non-empty gaps. Request
correlation is carried by the HTTP response header rather than duplicated in
the reasoning payload. Default empty coverage, conflict, ambiguity, and
truncation structures are not repeated. Byte streams use exact integrity
headers instead.

### Open

`POST /v1/workspace/open`

Request:

```json
{
  "task": "actual agent goal",
  "hints": {
    "authorization_scope": "scope:root",
    "root_refs": [],
    "open_object_refs": []
  },
  "resume_checkpoint_ref": "checkpoint:uuid",
  "token_budget": 24000
}
```

Behavior:

- Reject an empty task.
- Clamp token budget to 1,000 through 64,000.
- Return a new opaque session reference for client correlation only.
- Return current workspace generation.
- Run bounded exact and lexical candidate lanes, plus semantic only when the
  default-off runtime policy enables it.
- Treat exact+lexical as the required barrier for mixed retrieval: include
  semantic evidence when already ready, but never extend or downgrade the
  response for an optional semantic result.
- Wait for semantic under the 2.5-second retrieval bound only when the caller
  explicitly requests semantic alone.
- Preserve results from successful required lanes when another required lane
  fails.
- Group chunks by entry.
- Hydrate at most 12 coherent entries under the requested budget.
- When resuming, return the exact checkpoint and up to 200 changed paths after
  its generation.
- Do not calculate total record counts, corpus maps, or full manifests.

Evidence fields:

- entry reference
- path and title
- current version
- `complete_source` or `source_excerpt`
- source text
- heading when relevant

SHA-256 remains available through exact read and manifest. Ranking scores and
successful lane names are operational diagnostics, not default model input.

### Search

`POST /v1/workspace/search`

Accept one query or up to 16 batched queries. At most four queries execute at
once, bounding database pressure while avoiding sequential 16-lane latency.
After semantic readiness succeeds, distinct uncached semantic query embeddings
from the request are coalesced into bounded multi-input provider calls without
changing per-query deadlines or result order. Each query has optional ID, goal,
limit from 1 through 50, and modes selected from:

- `exact`
- `lexical`
- `semantic`

Default mode uses all policy-enabled lanes. Candidate work is bounded before entry
hydration. Results are merged by entry and include enough exact identity to
read the source. Lane failure produces a partial or degraded response, not loss
of successful candidates.

Exact mode resolves explicit quoted or backticked relative paths plus exact
path or title text. Lexical mode checks the 256 most recently changed entries
first. When fewer than 128 recent entries match, it also queries a bounded
candidate set from the full FTS GIN index so sparse recent leads do not hide
older authority. Dense broad queries remain recent-bounded. It may add at most
two explicit quoted or compound-identifier anchors from a natural-language
task. Semantic mode uses the HNSW index through the validated transaction user
context.

### Read

`POST /v1/workspace/read`

Accept 1 through 32 requests. Every request supplies an exact `path` or
`entry:<uuid>`.

Views:

- `full`
- `current_state`, an alias for current full text
- `outline`
- `range`, with 1-based inclusive line bounds

The response includes exact path, entry reference, version, SHA-256, media
type, text, non-empty metadata, and updated time. A missing exact target is not
replaced with search output. One response returns at most 4 MiB characters of
source text; when a batch reaches that budget, it reports the number of exact
requests to issue in a follow-up call.

A mixed-validity batch returns the valid entries and reports each missing path
or reference inline. The envelope is `partial` when at least one requested
entry succeeds and `degraded` only when none succeeds.

### Write

`POST /v1/workspace/write`

Request:

```json
{
  "path": "Projects/Example/Status.md",
  "content": "# Status\n",
  "media_type": "text/markdown",
  "expected_version": 3,
  "idempotency_key": "optional caller key",
  "metadata": {}
}
```

Rules:

- Path must be safe, relative, and at most the configured path length.
- Content is limited to 4 MiB.
- Media type is Markdown or plain text.
- `expected_version` must equal the current version; zero means create.
- Equal content is a no-op.
- A changed write appends one version and one change.
- Only that entry's current chunks are replaced.
- Exact and lexical search are ready when the write returns.
- Semantic embedding is always queued as bounded background work.
- A path cannot change between Markdown and binary kind implicitly.

### Capture

`POST /v1/workspace/capture`

Capture accepts durable text, source metadata, intent, and optional idempotency
key. It writes one Markdown file under a date-partitioned `Inbox/Captures/`
path and returns the normal write receipt. It does not create hidden claims,
objects, relations, or state machines. With an idempotency key, the capture
uses a stable create-only path: an exact replay is a no-op and different
content receives a conflict.

### Human-facing documents

`POST /v1/workspace/documents/publish`

Publishes or revises one intentionally human-facing Markdown document. The
request supplies a stable lowercase `slug`, plain `title`, Markdown `body_md`,
optional summary and typed provenance links, and optional idempotency and
expected-version guards. The service writes `Documents/<slug>.md` through the
normal versioned Markdown pipeline and returns the absolute authenticated
`/documents/<slug>` URL in `url`, plus the newly written revision in
`version_url`. Agents return the stable `url` by default; the pinned URL is for
an explicitly requested historical revision. Equal content and metadata are a
no-op; changing the body or human-facing metadata appends a revision.

`GET /v1/workspace/documents/<slug>?version=<n>`

Returns the current published version by default, or one exact historical
published version when requested, including title, body, freshness timestamps,
revision information, provenance, and both stable and version-pinned URLs.
Only versions marked `kind: human_document` are eligible. A raw import,
capture, generic Markdown entry, or unmarked historical predecessor is never
served through this route. Documents are private and require normal read
authorization. There is intentionally no list endpoint or navigation library.

### Changes

`GET /v1/workspace/changes?since_generation=<n>&limit=<n>`

Returns ordered changed paths after the cursor, current generation, and a
truncation flag. Limit is 1 through 2,000. Consumers continue from the last
returned generation.

### Checkpoint

`POST /v1/workspace/checkpoint`

A request supplies session ID, optional parent checkpoint, state, exact source
references, and optional idempotency key.

An explicit idempotency key opts into durable operation replay. The service
binds that key to the canonical parent, state, and source-reference payload;
`session_id` is correlation only in this mode. Reusing the key with the same
payload from a later session returns the original receipt, while reusing it
with a changed payload returns `409 idempotency_conflict`.

When a direct API client omits the key, the service derives a bounded implicit
key from the legacy deterministic checkpoint identity. This fallback preserves
the historical contract: an exact retry with the same body and session replays
the checkpoint, while a changed payload or session creates a distinct
checkpoint. Keys matching the generated `implicit:<uuid>` shape are reserved
for this fallback. MCP `memory.checkpoint` requires an explicit key and
therefore always uses durable cross-session replay semantics.

The service:

- derives a legacy-compatible deterministic checkpoint ID from the request
- resolves at most 64 explicit source paths or entry references
- rejects a missing explicit workspace reference rather than silently omitting
  it
- records their exact path, version, and hash
- writes one immutable Markdown entry under `.straylight/checkpoints/`
- appends only that entry's normal version, chunks, and change
- commits the entry, generation, embedding job, and exact replay receipt in one
  transaction
- returns the original logical receipt on replay, with the current request's
  session ID in the response envelope

Checkpoint state supports objective, current state, decisions, open questions,
next actions, and artifacts. Unknown fields may be retained in a JSON appendix
but do not create database schema.

### Binary Upload

`POST /v1/workspace/binaries`

Multipart fields:

- `file`, exactly one
- `path`
- `expected_content_hash`, required lowercase SHA-256 of the exact upload bytes
- optional `media_type`
- optional `description`
- optional `provenance`
- optional `limitations`
- optional portable metadata
- optional `expected_version`

The service hashes the uploaded object and refuses publication when it differs
from `expected_content_hash`. It then publishes the binary entry and Markdown
companion in one short database commit. No distributed transaction is
attempted. A failed database commit may leave an unreferenced object. The
alpha retains such objects because an automatic reference-check/delete loop
can race a valid publish; object cleanup is not part of request correctness.

The owner-alpha workspace upload and import path accepts a maximum individual
binary size of 4 GiB. Larger files fail before workspace publication. Supporting
them later requires a separate resumable multipart transport; it does not
change entry publication or retrieval semantics.

If no description is supplied, the companion says description is pending and
a `describe_binary` job is queued. This must not block upload completion.

### Binary Read

- `GET /v1/workspace/binaries`
- `GET /v1/workspace/binaries/<entry-ref>`
- `GET /v1/workspace/binaries/<entry-ref>/content`

List and metadata return exact entry, version, hash, size, media type,
description state, and portable metadata. Content fetch streams the exact
object version and returns expected hash, entry, version, and byte-length
headers. CLI and MCP clients verify SHA-256 while writing the stream; the API
does not buffer a large object merely to hash it before delivery.

Historical content fetch accepts an exact version. It never silently serves
current bytes for a historical request.

### Manifest

`GET /v1/workspace/manifest?limit=<n>&history=<bool>`

Returns a path-ordered page of current entries and workspace generation.
Current pages are limited to 5,000 entries. With `history=true`, each result is
an exact version row suitable for lossless export; pagination remains bounded.
When another page exists, `next` contains `after_path`, `after_entry_ref`, and,
for history, `after_version`. Clients should pass those values on the next
request. Offset remains a compatibility input for shallow interactive pages;
bulk transfer uses the keyset cursor.
Manifest is an explicit transfer and audit operation and is never calculated
by `open`, `search`, or `checkpoint`.

### Usage

`GET /v1/workspace/usage`

Sort options:

- `most_used`
- `least_used`
- `most_recently_used`
- `least_recently_used`

Each row includes path, kind, read count, search count, total uses, and first
and last timestamps. Usage recording is content-free, batched, and fail-open.

## CLI And MCP

The Rust `carrystate` CLI and TypeScript MCP server are thin clients over the
same workspace HTTP API. They do not implement alternative retrieval or write
semantics.

Primary MCP tools:

- `memory.open`
- `memory.query`
- `memory.read`
- `memory.capture`
- `memory.write`
- `memory.checkpoint`
- `memory.stage`
- `memory.status`
- `asset.list`
- `asset.metadata`
- `asset.fetch`
- `briefing.publish`
- `briefing.dedupe`
- `briefing.topics`
- `document.publish`
- `document.get`
- `notification.publish`

`document.publish` is request-directed: agents use it when the user asks to
show, open, or read a polished plan, specification, detailed analysis, travel
information, or comparable long-form material. Routine replies, raw imports,
and internal evidence are not automatically published.

`memory.compute` and `memory.verify` are not service tools. Agents use their
native reasoning, shell, browser, SQL, and code tools after retrieving exact
source material.

The MCP default response is compact JSON. It must not duplicate the same
payload as both text and structured content, and it must omit transport
metadata that is not useful for reasoning.

### Compatibility And Evaluation APIs

The workspace API above is the production product surface. Retained legacy
memory, account-export, and evaluation bulk-import routes are development and
test compatibility surfaces:

- development may enable them explicitly;
- production disables them by default;
- evaluation routes are always disabled in the production Compose profile;
- a simple-workspace evaluation identity may call `DELETE` on only its own
  evaluation import status URL; this evaluation-only transaction removes its
  searchable fixture state and jobs, soft-deletes its entries, and revokes its
  credentials without granting general credential-management authority;
- an operator must make a deliberate configuration change to expose legacy
  routes in production.

CLI, MCP, and evaluation harness defaults use the workspace protocol. A caller
cannot accidentally select the old corpus-revision and replay-ledger behavior
merely by omitting a protocol flag.

## Import

Import must:

- walk without following symlinks
- reject traversal, special files, and normalized path collisions
- sort paths deterministically
- hash exact bytes
- record size, media type, modification time, and portable mode
- classify valid UTF-8 Markdown/plain text as text and everything else as
  binary
- derive bounded attachment context from Markdown links
- treat each completed server write as durable resume progress without a
  duplicate per-file receipt ledger
- skip a path only when current server hash, byte length, and portable metadata
  equal the inventory
- correct changed portable metadata without creating another identical content
  version
- resume safely after interruption
- verify server path, hash, size, and companion state after upload

No import transaction may contain the full vault. Failure of one file leaves
prior completed files usable. Default import never deletes absent server
paths.

Mirror deletion requires:

1. A complete server-backed difference preview.
2. A confirmation hash over that exact preview.
3. Explicit caller confirmation.
4. Independent bounded deletes after uploads succeed.

## Export

Export must:

- refuse to overwrite an existing destination
- page through the explicit manifest
- download each exact text or object version
- verify SHA-256 and byte length
- preserve original relative path
- restore portable modification time and mode
- optionally include history under deterministic version paths
- write a machine-readable manifest and `CHECKSUMS.sha256`
- verify the completed staging directory before renaming it into place

Current export must round-trip every current path and byte exactly. History
export must round-trip every selected version and exact object-store version.
Each manifest row is an independent exact export promise. Export does not
require the whole workspace generation to remain unchanged while it runs.

## Jobs And Dreaming

Job kinds:

- `embed_entry`
- `describe_binary`
- `dream_workspace`

Jobs are claimed with `SKIP LOCKED`, have bounded attempts, backoff, lease
recovery, and a short sanitized error. A job never holds a database transaction
while calling OpenAI or transferring a large object.

Embedding requests may be batched for provider efficiency, but database
publication is limited to 128 chunks per independent statement, with no
enclosing document or job transaction. A partially published job is healthy
degraded state, not corruption. Retrying the job embeds only chunks whose
vector is still absent.

Dreaming reads changes after a per-user watermark and produces ordinary
Markdown patches. Low-risk maintenance includes indexes, summaries, stale-link
repair, people dossiers, event rollups, and task/briefing views. Consequential
or ambiguous changes are written as proposals.

Every applied patch uses normal entry versions. The SPA shows path, diff,
sources, model, reason, result, and revert action. There are no shadow corpus
revisions or automatic promotion gates.

## Data And Failure Semantics

- Current Markdown wins over historical checkpoints.
- Owner corrections win over older derived summaries.
- Source authority remains visible in Markdown and is not replaced by retrieval
  score.
- Empty retrieval is not proof of absence.
- A healthy dependency check is not proof of working retrieval.
- Embedding failure degrades to exact and lexical retrieval.
- One failed search lane does not discard other lanes.
- Metrics, usage, cleanup, and derivative maintenance fail open.
- Ordinary reads and writes do not wait for background repair.
- Unreferenced S3 objects are retained during the alpha. Any later lifecycle or
  lease-based cleanup remains asynchronous and outside workspace operations.

## Acceptance Gates

### Reasoning

- Aggregate paired claim recall across the full frozen harness is at least
  direct Markdown.
- Before claiming superiority or approving launch, an untouched frozen holdout
  is run once against the exact fingerprinted release candidate.
- No supported workload loses material source authority, correction, temporal,
  action-boundary, or current-over-history behavior.
- Recent Europe planning and Aether operating cases meet the same bar.
- Fresh-agent checkpoint resume advances work without reconstructing the whole
  workspace.
- Every suite remains visible separately; aggregate parity cannot hide a
  material lane-specific regression.

### Tokens

- Uncached model input is no worse than direct Markdown by more than the
  explicitly accepted quality premium.
- Protocol wrapper overhead is less than source text plus exact provenance
  identity for ordinary reasoning packets.
- No response includes corpus inventory or ranking diagnostics by default.

### Performance

At 1K, 10K, and 64K deterministic files:

- all searches and exact reads return the target
- open p95 is at most 5 seconds
- search p95 is at most 3 seconds
- exact read p95 is at most 1 second
- checkpoint is at most 2 seconds
- resume is at most 5 seconds
- checkpoint adds at most 100 database rows and 4 MiB
- open and search growth above 6x fails when the 64K p95 is also above 1 second;
  faster absolute results remain green and still report the observed ratio

A production-shaped test must also prove:

- lexical candidates use the GIN index
- the semantic lane degrades cleanly and becomes healthy after provider restore
- no normal open computes a whole-workspace count or map
- one failed lane still returns another lane's evidence
- concurrent unrelated writes do not serialize globally
- foreground write p95 stays below 3 seconds across at least 30 samples while
  semantic indexing has a large pending backlog
- a large import does not force PostgreSQL checkpoints more often than once per
  minute after the configured write-ahead-log budget is applied

### Fidelity

- Mixed Markdown and binary fixtures round-trip with identical paths, hashes,
  sizes, bytes, and portable metadata.
- Interrupted import resumes without reuploading completed equal hashes.
- Historical binary fetch returns the requested provider version.
- Every binary has a searchable companion.

### Security

- Cross-user read, search, binary fetch, manifest, usage, and write attempts
  fail.
- Read-only tokens cannot create any workspace row.
- Candidate functions derive user only from validated transaction context.
- Logs and metrics contain no bearer token, query text, source text, or direct
  personal identifier.

### Operations

- Authenticated behavioral readiness opens, searches, exact-reads, writes a
  disposable canary, and verifies cleanup.
- Backup and restore drills preserve PostgreSQL and exact object versions.
- Deployment identifies an immutable source revision and image digest.
- Datadog exposes latency, lane failure, job age, import progress, and per-user
  storage/entry growth before alpha migration.

## Explicit Non-Goals

- Distributed transactions across PostgreSQL and S3
- Whole-workspace snapshot isolation for ordinary agents
- Typed claim, relation, recurrence, or state-machine database hierarchies
- Service-side general computation or claim verification
- Transcript replay as the primary memory model
- Hidden model-written knowledge that users cannot inspect or revert
- Per-user databases or containers

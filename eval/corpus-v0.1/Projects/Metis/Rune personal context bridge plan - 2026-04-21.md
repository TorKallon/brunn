Updated: 2026-04-21 16:31 PDT

## Related
- [[Projects/Metis/Metis|Metis]]
- [[Projects/Metis/Rune bridge API handoff - 2026-04-21|Rune bridge API handoff]]
- [[Projects/Treehouse/Treehouse|Treehouse]]
- [[Active projects]]

## Goal
Let Rune access a very small, explicitly-approved slice of personal context so it can:
- show urgent personal todos while Rourke is at work
- contribute personal context to a unified morning brief
- do this without exposing Nyx, the raw vault, or broad personal credentials

## Recommendation
Use a scheduled outbound snapshot bridge for reads, plus a controlled writeback queue for task completion markers.

Best shape:
1. Nyx gathers and minimizes approved personal data locally on a schedule.
2. Nyx signs and pushes a small structured payload to a Vercel ingest endpoint over HTTPS.
3. Vercel verifies signature + replay protection, stores the curated payload, and exposes narrow read endpoints for Rune.
4. Rune can send back task completion markers to the broker.
5. Aether on Nyx validates those markers and applies the matching completion to Todoist.
6. Rune never gets direct access to Todoist, the raw Obsidian vault, or Nyx.

This is the simpler v1 and still gives you the important collaboration path: Rune can see curated personal state and hand completed items back to me for synchronization.

## Why this is the right default
- No inbound access to Nyx.
- Much simpler than maintaining a live command channel.
- Filtering still happens on Nyx before data leaves home.
- Rune only sees derived fields, not source systems.
- Writeback stays mediated by Aether instead of giving Rune direct Todoist authority.
- Easy to host on existing Vercel + `rourkem.com` setup.

## Recommended system design
### 1) Nyx exporter
Create a small exporter job on Nyx that runs on a schedule, for example every 15 minutes plus a pre-morning-brief run.

Its job is to:
- read approved local sources
- minimize and redact locally
- sign the payload
- push the latest curated snapshot to the broker
- also poll for pending Rune writeback markers and apply approved ones to Todoist

Allowed sources should still be explicit:
- Todoist: top urgent personal tasks only
- News: the already-curated personal/news briefing output or a structured summary source
- Obsidian vault: only specific generated views or allowlisted notes, not raw vault search

The exporter should produce a narrow JSON payload such as:

```json
{
  "generated_at": "2026-04-21T17:45:00Z",
  "todos": [
    {
      "id": "todoist:123",
      "content": "Schedule Radley ski tune",
      "priority": 1,
      "due": "2026-04-21",
      "bucket": "overdue"
    }
  ],
  "personal_brief": {
    "top_lines": [
      "Jen dentist appointment tomorrow 9:00 AM",
      "Radley tournament travel still unsettled"
    ]
  },
  "vault_context": {
    "active_projects": [
      {
        "name": "Metis",
        "status": "active",
        "next_step": "finish Drive ingestion pilot"
      }
    ]
  }
}
```

### 2) Vercel personal-data broker
Do not make the main Rune app talk directly to raw storage.

Instead create a dedicated broker service on Vercel, ideally as its own project. Good options:
- `bridge.rourkem.com`
- `personal-api.rourkem.com`
- or a separate Vercel project attached under `rourkem.com`

Broker responsibilities:
- verify signed snapshots from Nyx
- reject stale/replayed requests
- store only the curated payload
- serve narrow read endpoints for Rune
- accept narrow writeback markers from Rune
- keep an audit trail of what was ingested, requested, and written back

### 3) Rune access pattern
Rune should call broker endpoints like:
- `GET /api/rune/todos/urgent`
- `GET /api/rune/brief/personal`
- `GET /api/rune/context/active-projects`
- `POST /api/rune/todos/complete-marker`

These endpoints should return or accept only approved derived fields.

Rune should not get:
- raw vault note text by default
- Todoist API tokens
- Obsidian paths outside the allowlist
- direct database/blob credentials

### 4) Todoist writeback markers
Rune should not complete Todoist tasks directly.

Instead, when Rune believes a personal task is complete, it should submit a completion marker like:
- source task id
- completion confidence or reason
- optional note like "completed during work morning review"
- timestamp

Then Nyx/Aether should:
1. validate that the task id belongs to an exposed/allowed task
2. confirm the task is still open
3. optionally match by stable id plus content checksum
4. close it in Todoist
5. record the result back to the broker for audit

That keeps the authority to mutate personal systems on Nyx, via me, instead of in Rune.

## Security model
### Auth from Nyx to broker
Use request signing, preferably Ed25519.

- Nyx keeps the private key locally, ideally in macOS Keychain.
- Vercel stores the public key in environment config.
- Every snapshot upload includes:
  - timestamp
  - nonce
  - payload hash
  - signature
  - optional monotonic event id

Server checks:
- clock skew within a small window, for example 5 minutes
- nonce has not been used before
- event id has not already been accepted
- signature matches body

This is better than a long-lived bearer token alone.

### Authorization for Rune
Give Rune separate broker scopes for read and writeback.

Example scopes:
- `brief:read`
- `todos:read_urgent`
- `context:read_active_projects`
- `todos:mark_complete`

Important: `todos:mark_complete` should only create a completion marker. It should not directly mutate Todoist.

### Data protection
- TLS in transit.
- Encrypt sensitive stored payloads at the application layer if they contain anything more sensitive than task names and summaries.
- Do not log raw payload bodies.
- Add short retention by default, for example keep snapshots 7 to 30 days unless there is a real reason to keep more.

### Authorization for Rune
Give Rune its own broker credential with read-only scope.

Example scopes:
- `brief:read`
- `todos:read_urgent`
- `context:read_active_projects`

Do not create a general `personal:*` token.

## What data should be exposed
### 1) Todos
Expose only a filtered view, not the whole Todoist account.

Recommended first version:
- overdue personal tasks
- p1 tasks due today
- p1 tasks due tomorrow
- optionally p2 tasks due today if count is small

Recommended fields:
- stable task id
- content
- priority
- due date/time
- source project or label if useful
- short bucket like `overdue`, `today`, `tomorrow`

Recommended guardrails:
- denylist sensitive projects/labels
- or better, allowlist only the projects/labels intended for Rune visibility

### 2) News
Do not let Rune scrape arbitrary personal news sources through this bridge.

Better options:
- push the already-curated personal/news summary that Nyx uses for morning briefing
- or push a structured `top_headlines` list with title, source, url, why_it_matters

That keeps the personal/work unified brief consistent and avoids duplicate research stacks.

### 3) Vault context
Do not expose the raw vault.

Instead create explicit exported views on Nyx, such as:
- `active_projects`
- `family_upcoming`
- `important_waiting_on`
- `personal_morning_context`

Good first allowlist candidates:
- `Active projects`
- generated morning-brief note/output
- select project status notes
- select family logistics notes

Bad first candidates:
- health records
- financial records
- scanned documents
- free-form root vault search
- broad note retrieval across the whole vault

## Storage recommendation on Vercel
Use a small Postgres/Neon-backed table for structured broker data.

Suggested tables:
- `ingest_events`
- `personal_snapshots`
- `rune_access_log`
- `used_nonces`
- `task_completion_markers`
- `task_completion_results`

Use Blob only if later you want larger serialized snapshots or attachments.
For v1, structured relational storage is simpler.

## Suggested API shape
### Nyx ingest
- `POST /api/ingest/personal-snapshot`

Body:
- minimized structured JSON payload

Headers:
- `x-nyx-key-id`
- `x-nyx-timestamp`
- `x-nyx-nonce`
- `x-nyx-signature`
- `x-nyx-body-sha256`

### Rune read endpoints
- `GET /api/rune/todos/urgent`
- `GET /api/rune/brief/personal`
- `GET /api/rune/context/active-projects`
- optionally `GET /api/rune/morning-brief-input`

That last endpoint can return a single already-merged personal payload optimized for prompt injection into Rune's morning-brief workflow.

### Rune writeback endpoint
- `POST /api/rune/todos/complete-marker`

Body:
- task id
- observed completion state
- optional reason/note
- actor metadata
- timestamp

### Nyx writeback poll/apply flow
1. Nyx exporter uploads the latest snapshot.
2. Nyx checks for pending completion markers.
3. Nyx validates and applies allowed ones to Todoist.
4. Nyx writes back success/failure results to the broker.

## How this fits the current local setup
- `treehouse` already exists and backs `rourkem.com` on Vercel.
- `apps/rourkem-com` is a Next.js app and can host broker routes quickly.
- `metis` is the natural home for Nyx-side export and writeback code that reads curated Obsidian views.
- Todoist secrets already exist locally on Nyx.
- Metis is a better source substrate than the public site itself, because it already holds the assistant-facing context layer.
- This avoids adding a realtime relay layer in v1.

## Implementation plan
### Phase 1: read-only MVP
1. Create a dedicated broker service in the `treehouse` repo, ideally under `apps/rourkem-com` or a separate Vercel project.
2. Add a signed ingest endpoint.
3. Add a small Nyx exporter in `metis` that emits:
   - urgent todos
   - personal morning context
   - active projects summary
4. Store latest snapshot in Postgres.
5. Add one Rune-facing endpoint: `GET /api/rune/morning-brief-input`.
6. Test with fake/sample data first, then real filtered data.

### Phase 2: add Todoist completion markers
1. Add `POST /api/rune/todos/complete-marker`.
2. Store markers in a pending queue table.
3. Extend the Nyx exporter so each run also polls for pending markers.
4. Validate each marker against the latest exported task set.
5. Close matching tasks in Todoist via Nyx/Aether.
6. Write result rows back to the broker.
7. Expose marker status for audit/debugging.

### Phase 3: optional richer features
1. Split out separate endpoints for todos, briefing, and project context.
2. Add stricter scope controls and better admin/debug views.
3. Add additional writeback actions only after the completion-marker flow proves safe.
4. Attachment/blob support for specific safe artifacts if later needed.

## Strong guardrails
- No inbound port exposure on Nyx.
- No direct vault search endpoint for Rune.
- No raw personal credentials on Vercel beyond what the broker itself needs.
- No work AI access to health/finance/scanned-document domains in v1.
- No broad "just sync everything" design.

## Recommendation in one sentence
Build a narrow personal-data broker on Vercel, fed by an outbound signed exporter from Nyx, and let Rune send completion markers back through the broker so Aether on Nyx can safely sync them to Todoist.

## Concrete next step
Build the MVP around one read endpoint and one writeback endpoint:
- Nyx exports `urgent_todos + personal_morning_context + active_projects`
- Vercel stores the latest snapshot
- Rune reads `GET /api/rune/morning-brief-input`
- Rune writes `POST /api/rune/todos/complete-marker`
- Nyx/Aether validates and applies completion markers to Todoist on the next sync run

That gets the morning-brief use case working quickly and adds the collaboration loop without overexposing your personal systems.

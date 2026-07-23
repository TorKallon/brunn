Updated: 2026-04-21 16:31 PDT

Status: live on `https://rourkem.com`, with production env wiring applied on Vercel and smoke-tested read + marker queue flow against the live domain.

## Related
- [[Projects/Metis/Metis|Metis]]
- [[Projects/Metis/Rune personal context bridge plan - 2026-04-21|Rune personal context bridge plan]]
- [[Projects/Treehouse/Treehouse|Treehouse]]
- [[Active projects]]

## What Rune should use
Base URL:
- `https://rourkem.com`

Auth:
- Header: `Authorization: Bearer <shared-secret>`

## Endpoints Rune can call
### `GET /api/personal-bridge/rune/morning-brief-input`
Returns the current curated personal payload.

Response shape:
```json
{
  "generatedAt": "2026-04-21T18:00:00.000Z",
  "urgentTodos": [
    {
      "sourceId": "todoist:abc123",
      "content": "Book ski tune",
      "priority": 4,
      "priorityLabel": "p1",
      "dueDate": "2026-04-22",
      "dueDatetime": null,
      "bucket": "tomorrow",
      "projectId": "proj123",
      "projectName": "Personal",
      "labels": []
    }
  ],
  "personalBrief": {
    "sourcePath": "Briefings/Morning briefing - 2026-04-21.md",
    "excerpt": "..."
  },
  "vaultContext": {
    "notes": [
      {
        "path": "Active projects.md",
        "title": "Active projects",
        "excerpt": "..."
      }
    ]
  }
}
```

### `GET /api/personal-bridge/rune/todos/urgent`
Returns just the exported urgent todo set.

### `GET /api/personal-bridge/rune/context/active-projects`
Returns just the exported vault-context notes.

### `POST /api/personal-bridge/rune/todos/complete-marker`
Queues a completion request for Nyx to validate and apply.

Request body:
```json
{
  "taskId": "todoist:abc123",
  "note": "Completed during morning review",
  "requestedBy": "rune"
}
```

Success response:
```json
{
  "ok": true,
  "markerId": "marker_...",
  "status": "pending",
  "taskId": "todoist:abc123",
  "taskContent": "Book ski tune"
}
```

## Rules Rune should assume
- Rune only sees the latest exported snapshot, not the raw vault.
- Rune must only submit `taskId` values it received from the broker.
- `complete-marker` is asynchronous. It is a request for Nyx to review/apply, not direct Todoist authority.
- If a task drops out of the exported snapshot before Nyx processes it, Nyx may reject it.

## Nyx-side flow
- `metis/scripts/rune-personal-bridge.js sync`
- Uploads the latest curated snapshot
- Fetches pending completion markers
- Validates them against the latest exported tasks
- Closes matching Todoist tasks on Nyx
- Posts result status back to the broker

## Files changed
### treehouse
- `apps/rourkem-com/app/api/personal-bridge/...`
- `apps/rourkem-com/lib/personal-bridge/...`
- `docs/rune-personal-bridge.md`

### metis
- `scripts/rune-personal-bridge.js`
- `src/rune-bridge/...`
- `docs/rune-personal-bridge.md`

## Production status
Done on the `rourkem.com` Vercel project:
- `PERSONAL_BRIDGE_RUNE_SHARED_SECRET` set
- `PERSONAL_BRIDGE_INGEST_SECRET` set
- private Vercel Blob store `personal-bridge` linked, which provided `BLOB_READ_WRITE_TOKEN`
- production deploy completed successfully
- live snapshot upload succeeded
- live `GET` read path and `complete-marker` queue path smoke-tested successfully

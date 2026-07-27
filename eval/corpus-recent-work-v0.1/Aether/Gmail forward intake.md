# Gmail forward intake

The deterministic triage script is the source of truth for each run.

No-action behavior:

- If `actionable` is empty and there are no meaningful errors, return exactly
  `No eligible new inbox messages.`
- The same response applies when the only results were routine no-op archives.
- Never return an empty response.

Actionable forwarded mail:

- Only forwarded messages from `owner@example.com` are eligible.
- Treat text above the forwarded content as the owner's instruction.
- Search the Family calendar around the same date, time, and title before
  creating an event.
- Download attachments to a unique per-message folder and leave originals in
  place after processing.
- After successful handling, mark the message read and archive it.
- If the request is ambiguous, risky, or blocked, leave it unread in the inbox.

Do not redo broad mailbox triage manually unless the script itself failed.

# Brunn iOS briefing, tasks, and notifications MVP

Status: briefing/account-session baseline installed as build 4; bounded Tasks
navigation and actions implemented in the current source; notification
production rollout and signed-device canaries pending, 2026-08-28

## Product outcome

The iOS MVP is a focused native client for content published by Brunn's
briefing agents, bounded agent-first task action, and durable alerts. It is not
a WebView, general assistant shell, unbounded task manager, or second notes
database.

The primary design rule is simple: use the phone for prose. The native reader
follows the deployed mobile web view with 12-point phone gutters, no decorative
timeline rail, a full-width summary with a three-point Still Water accent
(signal blue), compact section rows, and full-width expanded detail. Dynamic
Type, VoiceOver, selection, Reduce Motion, and safe web links are preserved.

## Information architecture

| Surface | Responsibility |
| --- | --- |
| Home | Dashboard plus source-backed workspace entry search with selectable Best match, Last modified, and Title order; exact version-pinned reads; formatted/raw Markdown; and safe internal entry links. |
| Today | Briefing-only current structured edition, complete summary, every section and item, legacy Markdown fallback, sources, timestamps, change context, and revision disclosure. |
| Tasks | Dedicated bounded Urgent, Next, Done today, and Projects surface with completion, snooze, provenance, detail, and content-free Todoist status. |
| Agents | Conditional messaging surface when the server enables agent messaging. |
| Alerts | Durable notification events across briefings, material news, corrections, and operational attention, with server-backed unread/acknowledged state and exact detail. |
| Archive | Newest-first cursor-paginated editions, date/edition navigation, exact current or pinned historical versions, and the complete reader. |
| Settings | Appearance, connection/cache/privacy state, contextual notification permission, and installation controls. |

Tasks has its own direct bottom-tab destination; it does not occupy Today.
Native tab overflow may place secondary destinations under More without moving
Tasks behind overflow. The connected app uses hosted Brunn as the source
of truth. The latest edition and bounded task surface are cached as disposable,
account-bound, data-protected snapshots, never as an offline task database or
mutation queue. Alert state remains server-backed and independent of APNs
success. The push is only an attention signal; authenticated detail is the
durable record.

## Current client contract

The production API base is `https://brunn.ai/api/v1`.

| Client job | Endpoint |
| --- | --- |
| Create and validate the account session | `POST /auth/login`, `GET /auth/session`, `GET /me` |
| List and paginate editions | `GET /workspace/briefings?limit=&after_path=` |
| Read an edition or pinned version | `GET /workspace/briefings/{date}/{edition}?version=` |
| Read topics and pending deep-dives | `GET /workspace/briefings/topics` |
| Model briefing actions | `POST /workspace/briefings/items/action` |
| Search workspace entries | `POST /workspace/search` with per-query `sort=best_match|last_modified|title` |
| Read an exact current or pinned entry | `POST /workspace/read` with exact `ref`/`path` and optional `version` |
| Read bounded task candidates and detail | `GET /workspace/tasks/candidates?view=`, `GET /workspace/tasks/{task_ref}` |
| Complete, snooze, or otherwise update one task | `PATCH /workspace/tasks/{task_ref}` |
| Read task support data | `GET /workspace/tasks/done-summary`, `GET /workspace/contexts`, `GET /workspace/projects`, `GET /workspace/projects/{slug}/state` |
| Read content-free Todoist state | `GET /workspace/integrations/todoist/status` |
| Enable or revoke exact device action access | `POST /credentials`, `DELETE /credentials/{credential_ref}` |
| List/read durable alerts | `GET /workspace/notifications`, `GET /workspace/notifications/{notification_ref}` |
| Record open/acknowledgement | `POST /workspace/notifications/{notification_ref}/receipts` |
| Register/revoke this installation | `PUT /workspace/notification-installations/{installation_id}`, `DELETE /workspace/notification-installations/{installation_id}` |

The client decodes the complete deployed `briefing.v1` payload: summary,
sections, headlines, bodies, detailed prose, why-it-matters, what-changed,
delta state, sources, published/event/first-seen timestamps, and version
history. Date-only event values are formatted as calendar dates without a
timezone shift. Only HTTP(S) sources become tappable links.

Alerts decode typed notification/source/target records. A push tap resolves an
opaque notification reference through the authenticated API, records a
delivery-attributed open when available, and displays durable detail before it
offers a secondary action to Today, an exact briefing item, or a pinned entry.

Entry search defaults to server-ranked Best match and can be rerun in
newest-modified or title order. Every result carries the exact entry reference,
version, and modification time used by the reader; relevance scores remain a
server-side ranking detail. Opening a result pins that version. The reader
defaults to formatted Markdown and can reveal the raw source without another
network read. HTTP(S) links open externally; relative
Markdown links and `[[wiki links]]` resolve inside Brunn through canonical
paths, hosted `sources/` vault-root expansion, and a server-confirmed unique
basename lookup for bare wiki links. Ambiguous links and other URL schemes fail
closed.

Tasks defaults to the deterministic, globally bounded ready set: conditional
Urgent, at most five unique Next rows plus pins, Done today, and Projects. iOS
reads the server-owned `surface_defaults["ios"]` context availability on every
refresh and filters it against the active registry. There is no persistent
context strip or device-local context override; owners or agents change the
default through Web settings or the task-context management tool. Imported
rows carry a Todoist provenance marker. The Todoist card is deliberately
content-free: it shows only environment/saved/effective modes, a
token-configured boolean, configuration generation, run timestamps/outcome,
and an error code—never a token, task text, or external identifier.

## Authentication and mutations

The app signs in with the same email and password as the web UI. The password
is sent only to `POST /auth/login` and is never stored by the app. Hosted
Brunn returns the same revocable, 30-day `HttpOnly` session used by the
web app; iOS persists that cookie across launches and sends the readable CSRF
cookie as `X-CSRF-Token` on unsafe methods. Logout clears both server and local
session state. “Disconnect this iPhone” first revokes its notification
installation, then logs out and clears local state. Registrations survive
normal session rotation and password reset; account deletion cascades through
the user's installations, deliveries, and receipts. Requests use HTTPS with
response caching disabled.

Task reads use the owner cookie session. Task mutations never reuse that cookie:
they use a separate opaque Keychain bearer with the exact approved iOS profile,
`task.write` plus notification-management, and optionally `message.write` only
when agent messaging is enabled. It receives no workspace read, save,
checkpoint, integration-management, or other broad capability. If the bearer
is missing, task data stays visible but view-only; an inline **Enable task
actions** control creates the least-privilege credential in place. Mutations are
online-only, versioned, and idempotent; the app keeps no offline action queue.
Briefing item actions remain outside this narrow task-write contract.

## Push boundary

Brunn now has a durable inbox, installation registration, APNs outbox and
attempt state, and open/acknowledgement receipts. iOS asks permission only from
the contextual setup flow, obtains the current APNs token each launch, keeps it
in memory, and upserts it through the account session with CSRF protection.

The strict `brunn-push@v1` payload contains a bounded preview of an
operational alert's body. Briefing, material-news, and correction pushes retain
generic APS prose. Every payload otherwise contains only opaque
notification/delivery references and a matching typed route; paths, semantic
item identifiers, source URLs, and user identifiers stay out of APNs. APNs
`accepted_by_apns` is displayed honestly and never called device delivery.

The `BRUNN_APNS_ENVIRONMENT` build setting expands into the
`aps-environment` entitlement and the installation request. Debug builds use
Apple's sandbox and Release builds resolve to `production`; every signed
archive must still be checked against its provisioning profile. Device tokens
cannot cross those environments. The server's
configured app topic must remain `com.rourkem.brunn` for this target.

Production remote delivery remains gated on provider credentials and
signed-device canaries for morning, intraday, correction, retry, invalid token,
denied permission, cold/warm launch, and open attribution. Keep dual iMessage
delivery until those canaries pass.

## Verification gates

```sh
swift test \
  --package-path /Users/Shared/projects/brunn/apps/ios \
  --scratch-path /tmp/brunn-ios-spm

xcodebuild \
  -project /Users/Shared/projects/brunn/apps/ios/Brunn.xcodeproj \
  -scheme Brunn \
  -destination 'platform=iOS Simulator,name=RuptureOps iPhone 14' \
  test

xcodebuild \
  -project /Users/Shared/projects/brunn/apps/ios/Brunn.xcodeproj \
  -scheme Brunn \
  -sdk iphoneos \
  -destination 'generic/platform=iOS' \
  CODE_SIGNING_ALLOWED=NO \
  build
```

UI coverage verifies the reader occupies at least 90% of the phone width,
seven-line summary collapse/restore, every section and source-backed detail,
revision history, briefing-only Today, the dedicated Tasks tab with no context
strip, inline view-only/action enablement, completion, snooze, content-free
Todoist status, task deep-link routing, Alerts filters/detail/acknowledgement/
target routing, and Archive version selection. Push contract coverage rejects
malformed or mismatched opaque references and verifies that a valid tap presents
durable detail before its target.

## MVP exclusions

- assistant chat and generated answers
- arbitrary workspace editing
- arbitrary unbounded task browsing or offline task mutation
- a whole-corpus local mirror or local embeddings
- notification preferences and quiet hours
- widgets, Watch, Live Activities, Siri, Spotlight, or Critical Alerts

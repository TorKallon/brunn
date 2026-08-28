# Straylight iOS briefing and notifications MVP

Status: briefing/account-session baseline installed as build 4; notifications
implemented locally with production rollout and signed-device canaries pending,
2026-08-02

## Product outcome

The iOS MVP is a focused native reader for content published by Straylight's
briefing agents. It is not a WebView, assistant shell, task manager, or second
notes database.

The primary design rule is simple: use the phone for prose. The native reader
follows the deployed mobile web view with 12-point phone gutters, no decorative
timeline rail, a full-width summary with a three-point Night Signal accent
(signal blue), compact section rows, and full-width expanded detail. Dynamic
Type, VoiceOver, selection, Reduce Motion, and safe web links are preserved.

## Information architecture

| Surface | Responsibility |
| --- | --- |
| Today | Current structured edition, complete summary, every section and item, legacy Markdown fallback, sources, timestamps, change context, and revision disclosure. |
| Search | Source-backed workspace entry matches with selectable Best match, Last modified, and Title order; exact version-pinned reads; formatted/raw Markdown; and safe internal entry links. |
| Alerts | Durable notification events across briefings, material news, corrections, and operational attention, with server-backed unread/acknowledged state and exact detail. |
| Archive | Newest-first cursor-paginated editions, date/edition navigation, exact current or pinned historical versions, and the complete reader. |
| Settings | Appearance, connection/cache/privacy state, contextual notification permission, and installation controls. |

The connected app uses hosted Straylight as the source of truth. Only the
latest edition is cached as a disposable, data-protected offline snapshot.
Alert state remains server-backed and independent of APNs success. The push is
only an attention signal; authenticated detail is the durable record.

## Current client contract

The production API base is `https://straylight.rourkem.com/api/v1`.

| Client job | Endpoint |
| --- | --- |
| Create and validate the account session | `POST /auth/login`, `GET /auth/session`, `GET /me` |
| List and paginate editions | `GET /workspace/briefings?limit=&after_path=` |
| Read an edition or pinned version | `GET /workspace/briefings/{date}/{edition}?version=` |
| Read topics and pending deep-dives | `GET /workspace/briefings/topics` |
| Model briefing actions | `POST /workspace/briefings/items/action` |
| Search workspace entries | `POST /workspace/search` with per-query `sort=best_match|last_modified|title` |
| Read an exact current or pinned entry | `POST /workspace/read` with exact `ref`/`path` and optional `version` |
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
Markdown links and `[[wiki links]]` resolve inside Straylight through canonical
paths, hosted `sources/` vault-root expansion, and a server-confirmed unique
basename lookup for bare wiki links. Ambiguous links and other URL schemes fail
closed.

## Authentication and mutations

The app signs in with the same email and password as the web UI. The password
is sent only to `POST /auth/login` and is never stored by the app. Hosted
Straylight returns the same revocable, 30-day `HttpOnly` session used by the
web app; iOS persists that cookie across launches and sends the readable CSRF
cookie as `X-CSRF-Token` on unsafe methods. Logout clears both server and local
session state. “Disconnect this iPhone” first revokes its notification
installation, then logs out and clears local state. Registrations survive
normal session rotation and password reset; account deletion cascades through
the user's installations, deliveries, and receipts. Requests use HTTPS with
response caching disabled.

The deployed item-action endpoint requires the broad `save` capability and has
no idempotency key. The iOS client includes the typed request/response contract
but does not enable automatic or interactive writes. A narrow mobile
briefing-interaction contract should precede Mark read, feedback, Go deeper,
or Mute topic server writes.

## Push boundary

Straylight now has a durable inbox, installation registration, APNs outbox and
attempt state, and open/acknowledgement receipts. iOS asks permission only from
the contextual setup flow, obtains the current APNs token each launch, keeps it
in memory, and upserts it through the account session with CSRF protection.

The strict `straylight-push@v1` payload contains a bounded preview of an
operational alert's body. Briefing, material-news, and correction pushes retain
generic APS prose. Every payload otherwise contains only opaque
notification/delivery references and a matching typed route; paths, semantic
item identifiers, source URLs, and user identifiers stay out of APNs. APNs
`accepted_by_apns` is displayed honestly and never called device delivery.

The `STRAYLIGHT_APNS_ENVIRONMENT` build setting expands into the
`aps-environment` entitlement and the installation request. Debug builds use
Apple's sandbox and Release builds resolve to `production`; every signed
archive must still be checked against its provisioning profile. Device tokens
cannot cross those environments. The server's
configured app topic must remain `com.rourkem.straylight` for this target.

Production remote delivery remains gated on provider credentials and
signed-device canaries for morning, intraday, correction, retry, invalid token,
denied permission, cold/warm launch, and open attribution. Keep dual iMessage
delivery until those canaries pass.

## Verification gates

```sh
swift test \
  --package-path /Users/Shared/projects/straylight/apps/ios \
  --scratch-path /tmp/straylight-ios-spm

xcodebuild \
  -project /Users/Shared/projects/straylight/apps/ios/Straylight.xcodeproj \
  -scheme Straylight \
  -destination 'platform=iOS Simulator,name=RuptureOps iPhone 14' \
  test

xcodebuild \
  -project /Users/Shared/projects/straylight/apps/ios/Straylight.xcodeproj \
  -scheme Straylight \
  -sdk iphoneos \
  -destination 'generic/platform=iOS' \
  CODE_SIGNING_ALLOWED=NO \
  build
```

UI coverage verifies the reader occupies at least 90% of the phone width,
seven-line summary collapse/restore, every section and source-backed detail,
revision history, Alerts filters/detail/acknowledgement/target routing, and
Archive version selection. Push contract coverage rejects malformed or
mismatched opaque references and verifies that a valid tap presents durable
detail before its target.

## MVP exclusions

- assistant chat and generated answers
- arbitrary workspace editing
- task lists or task mutation
- a whole-corpus local mirror or local embeddings
- notification preferences and quiet hours
- widgets, Watch, Live Activities, Siri, Spotlight, or Critical Alerts

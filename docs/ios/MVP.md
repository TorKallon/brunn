# Straylight iOS briefing and news MVP

Status: implemented and simulator-tested, 2026-08-02

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
| News | Version-derived briefing activity across recent editions, including new, updated, removed/correction, priority, and device-session unread views. |
| Archive | Newest-first cursor-paginated editions, date/edition navigation, exact current or pinned historical versions, and the complete reader. |
| Settings | Appearance, connection/cache/privacy state, and notification-readiness controls. |

The connected app uses hosted Straylight as the source of truth. Only the
latest edition is cached as a disposable, data-protected offline snapshot.
News read state is deliberately session-local because the server does not have
a delivery or acknowledgement ledger.

## Current client contract

The production API base is `https://straylight.rourkem.com/api/v1`.

| Client job | Endpoint |
| --- | --- |
| Create and validate the account session | `POST /auth/login`, `GET /auth/session`, `GET /me` |
| List and paginate editions | `GET /workspace/briefings?limit=&after_path=` |
| Read an edition or pinned version | `GET /workspace/briefings/{date}/{edition}?version=` |
| Read topics and pending deep-dives | `GET /workspace/briefings/topics` |
| Model briefing actions | `POST /workspace/briefings/items/action` |

The client decodes the complete deployed `briefing.v1` payload: summary,
sections, headlines, bodies, detailed prose, why-it-matters, what-changed,
delta state, sources, published/event/first-seen timestamps, and version
history. Date-only event values are formatted as calendar dates without a
timezone shift. Only HTTP(S) sources become tappable links.

Recent News activity is reconstructed from recent editions and up to five
versions per edition. Removed items become visible correction events instead
of disappearing silently. It is labeled briefing activity, never phone
delivery.

## Authentication and mutations

The app signs in with the same email and password as the web UI. The password
is sent only to `POST /auth/login` and is never stored by the app. Hosted
Straylight returns the same revocable, 30-day `HttpOnly` session used by the
web app; iOS persists that cookie across launches and sends the readable CSRF
cookie as `X-CSRF-Token` on unsafe methods. Logout clears both server and local
session state. Requests use HTTPS with response caching disabled.

The deployed item-action endpoint requires the broad `save` capability and has
no idempotency key. The iOS client includes the typed request/response contract
but does not enable automatic or interactive writes. A narrow mobile
briefing-interaction contract should precede Mark read, feedback, Go deeper,
or Mute topic server writes.

## Push boundary

Native remote delivery is not part of the currently deployable server
contract. Straylight has no device-registration, notification preference,
APNs outbox/attempt, inbox, open-receipt, or acknowledgement endpoint or table.
The app therefore does not consume the one-time notification prompt or claim
that briefing inclusion means an iPhone alert was delivered.

The required follow-on remains:

1. authenticated installation upsert/revoke with protected APNs token material;
2. private notification preferences and quiet hours;
3. an idempotent outbox and APNs attempt ledger;
4. an authenticated inbox resolved from opaque delivery identifiers;
5. open and explicit acknowledgement receipts;
6. signed-device canaries for morning, intraday, correction, retry, invalid-token, and quiet-hours cases;
7. dual delivery until those canaries prove the iMessage path can be retired.

Default lock-screen payloads must remain generic and contain no briefing prose,
paths, semantic identifiers, source URLs, or personal values.

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
revision history, News filters and session read state, and Archive version
selection.

## MVP exclusions

- assistant chat and generated answers
- arbitrary workspace editing
- task lists or task mutation
- a whole-corpus local mirror or local embeddings
- push claims without a server delivery ledger
- widgets, Watch, Live Activities, Siri, Spotlight, or Critical Alerts

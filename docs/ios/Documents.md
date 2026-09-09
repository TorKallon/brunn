# Native published-document links

Implemented and verified on September 9, 2026. Rourke authorized committing,
pushing, and coordinating the backend/MCP production release with the active
Brunn 2 task. Release revision and verified deployment outcomes are recorded in
Brunn's shared document-link handoff. Signed iOS distribution and real-device
Messages testing remain separate steps; source implementation is not proof of
an installed reader. The existing published handoff is planning/history.

## URL contract

| Link | Meaning |
| --- | --- |
| `brunn://document/<slug>` | Fetch the current intentionally published revision each time it opens. |
| `brunn://document/<slug>?version=<n>` | Fetch exactly revision `n`; never fall back to latest. |

The host is singular `document`. Scheme/host are case-insensitive; serialization
is lowercase. Slugs are 2–80 lowercase ASCII letters, digits, or hyphens, with
alphanumeric endpoints. They are not derived from a title or an `entry:` ref.
There is exactly one path component, no trailing slash, and no percent-encoded
path characters. At most one query parameter is accepted: `version` as canonical
decimal `1...9223372036854775807`, without signs, leading zeroes, escaping, or
extra delimiters. Unknown/duplicate parameters, empty queries, userinfo, ports
(even empty ones), fragments, encoded separators and traversal are rejected.

`DocumentLink` is the shared native parser, API request target, and serializer.
The backend uses `document_app_url` for publish/get/history; both implementations
test the same vectors in `contracts/document-links.json`. Existing Today,
Review/Dreams, dated briefing, notification, task, and conversation links retain
their prior contract. In particular `brunn://briefing/2026-09-09/morning` is still
a briefing, not a document.

## API and MCP

The reader uses the existing cookie-authenticated
`GET /v1/workspace/documents/<slug>[?version=<n>]` and workspace envelope. On
the production same-origin base that is
`https://brunn.ai/api/v1/workspace/documents/<slug>`.

`document.publish` and `document.get` pass through these additive fields:

| Field | Meaning |
| --- | --- |
| `app_url` | Stable latest native link. |
| `app_version_url` | Native link pinned to the returned revision. |
| `versions[].app_version_url` (get) | Native link pinned to that published history row. |
| `url`, `version_url`, `versions[].version_url` | Existing authenticated web links, unchanged. |

The new native client can decode older API responses without app-link fields;
it uses `DocumentLink` when sharing. This backward compatibility does not make
older installed iOS builds support document links. Agents should return the
server's actual `app_url` when asked for an app link and use pinned sharing only
explicitly. No document is republished as a briefing or migrated for routing.

## Reader and lifecycle

The existing `AppRoute` / `AppModel` pending-route lifecycle owns navigation.
The reader is a dismissible native sheet; there is no Documents tab. Initial
sign-in names the requested slug/revision and replays the destination after
validation. A warm link replaces the existing request. A new document, another
route, dismissal, connection reset, logout, or account change cancels and fences
old work. Request generations and account checks reject even non-cooperative
old responses; the API freezes cookies to the initiating session and rejects a
response if that session changed. Document bodies are not cached to disk.

Loading, authentication-required, unavailable/forbidden/not-found, and retryable
network failures stay honest. A mismatched response slug or revision is an
error, not a substitute reader. Pinned content always has a visible version
notice and **Open latest**; historical content is labelled as such. Copy and
Share default to the stable app link. Sharing the viewed revision is explicit.

SwiftUI and Foundation's Markdown parser preserve headings, inline emphasis,
nested lists, links, blockquotes, code and tables. Tables become labelled rows
on the narrow reading surface; code scrolls horizontally without truncation.
Dynamic Type is uncapped, headings expose VoiceOver header traits, text remains
selectable, and the app's persisted dark/light Still Water tokens apply. Reader
and location-setup sheets are serialized; no location permission policy changes.

SafeMarkdown allows external HTTP(S) without credentials and existing supported
app routes. Relative paths, raw `entry:` refs, private wiki references, filesystem
URLs, and unknown app destinations are not turned into links. Curated private
sources display their labels without guessing a URL or opening a generic entry
browser. The backend's owner and `human_document` / `document.v1` boundaries
remain authoritative and unchanged.

## Verification and distribution

Focused tests:

```sh
swift test --package-path apps/ios --scratch-path /tmp/brunn-document-links-spm
xcodebuild -project apps/ios/Brunn.xcodeproj -scheme Brunn \
  -destination 'platform=iOS Simulator,name=RuptureOps iPhone 14' \
  -only-testing:BrunnTests/DocumentReaderTests \
  -only-testing:BrunnUITests/DocumentReaderUITests test
(cd apps/api && cargo test --lib document_service::tests)
# Set BRUNN_TEST_DATABASE_URL to a disposable database, never an owner database.
(cd apps/api && cargo test --test document_endpoints)
(cd apps/mcp && npm run build && node --test dist/document-tools.test.js)
```

September 9 verification on Nyx (base `main` `99843e7`, document diff uncommitted):

- Swift Package: 48 passed, including seven document contract/API tests.
- Native iOS unit tests: 143 passed, including eleven document lifecycle/Markdown
  tests covering offline cached launch and a 30 KB-plus handoff.
- Document simulator UI tests: five passed on iPhone 14 / iOS 26.5, including
  cold/warm OS URL dispatch, historical/Open latest UI, stable/pinned sharing,
  unavailable revisions, real target slugs through sign-in, light/dark rendering,
  largest accessibility text, and contrast/text-clipping/trait audits. Screenshots
  were visually inspected. A SwiftUI Link test query was corrected to use the
  actual accessibility identifier.
- Rust document unit tests: nine passed. Disposable PostgreSQL + real HTTP
  router: two passed, including publish/get link fields, republishing, owner/RLS
  isolation, authentication, raw-entry exclusion, and exact historical reads.
- Full MCP build/tests: 111 passed, including text and structured app-link output.
- Strict Rust Clippy (`--lib --test document_endpoints -- -D warnings`) passed.
- Simulator build and unsigned generic iPhoneOS build passed. The wider native
  suite was rerun with normal simulator signing after unsigned execution denied
  Keychain access; all 143 then passed without changing location code.

Hosted read-only smoke checks confirmed `chief-of-staff-brief` (v42 at check time)
and `brunn-ios-document-deep-links-handoff` (v1) exist. The deployed response did
not yet return the new app fields. **No real-device Messages tap, authenticated
native read of those real documents, signed-device distribution, or production
deployment was performed.** Those remain release checks, not test successes.

Before calling the feature live:

1. Deploy the reviewed backend/MCP revision through the normal approval process
   and verify authenticated publish/get results return the additive fields.
2. Build/distribute an approved signed iOS build containing this reader.
3. On that installed build, tap the real chief-of-staff and handoff links in
   Messages, both cold and warm; read their full authenticated contents. Check a
   pinned revision, Open latest, and the existing dated briefing link.
4. Report simulator URL dispatch separately from a real-device Messages tap;
   a demo fixture or signed-out destination check is not a live-document read.

Smoke targets are `brunn://document/chief-of-staff-brief` and
`brunn://document/brunn-ios-document-deep-links-handoff`. No universal-link/AASA
setup, production monitoring, push changes, or generic raw-entry deep links are
included.

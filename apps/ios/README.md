# Brunn for iOS

Native SwiftUI home dashboard, briefing reader, bounded task client, and durable
Alerts inbox for hosted Brunn.

## Implemented MVP

- full-width Today reader modeled on the mobile web view
- dedicated bounded Tasks tab with Urgent/Next, completion, snooze, Done today,
  projects, source provenance, and content-free Todoist import status
- Home dashboard with direct memory-search access, storage totals, seven-day
  usage charts, and a credential access inventory
- workspace entry search with Best match, Last modified, and Title ordering,
  exact version-pinned reads, formatted/raw Markdown modes, and safe in-app
  navigation for relative Markdown and wiki links
- complete `briefing.v1` summary, sections, detail, sources, timestamps, and revisions
- server-backed Alerts with All, Important, and Unread filters, authenticated
  detail resolution, acknowledgement, and exact target navigation
- cursor-paginated briefing Archive with exact historical-version selection
- persisted dark/light appearance settings
- bounded protected latest-edition cache and persistent 30-day account session
- strict opaque push routes, in-memory APNs token forwarding, Dynamic Type,
  VoiceOver identifiers, and Reduce Motion support
- notification taps that fetch authenticated durable detail before offering a
  briefing or entry target
- deterministic demo fixtures plus Swift Package, app-unit, and UI tests

Search and its entry reader are reachable from Home. Today remains
briefing-only, while Tasks has a dedicated bottom-tab destination and applies
the server/agent-managed iOS context default without a persistent context strip.
Task reads use the owner Web session. An inline enablement control creates the
exact least-privilege device credential for mutations; absent that credential,
the same bounded task data remains view-only. Todoist status is content-free,
and import configuration stays in Web settings. The notification inbox,
installation, receipt, and APNs outbox contracts are implemented; production
push remains gated on provider configuration and a signed-device canary. See
`docs/ios/MVP.md` for the rollout boundary.

## Open

```sh
open /Users/Shared/projects/brunn/apps/ios/Brunn.xcodeproj
```

Launch with `--demo` to bypass the account sign-in screen and use the
deterministic briefing, alert, archive, and dashboard fixtures.

## Verify

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

The checked-in Xcode project mirrors `project.yml`. The app itself has no
third-party runtime dependencies. Build 4's email/password account-session
login remains the authentication baseline for notification registration,
detail, and receipts.

# Straylight for iOS

Native SwiftUI home dashboard, briefing, and news reader for hosted Straylight.

## Implemented MVP

- full-width Today reader modeled on the mobile web view
- Home dashboard with direct briefing/search access, storage totals, seven-day
  usage charts, and a credential access inventory
- complete `briefing.v1` summary, sections, detail, sources, timestamps, and revisions
- recent version-derived News activity with All, Priority, and Unread filters
- cursor-paginated briefing Archive with exact historical-version selection
- read-only tracked topics and pending deep-dive requests
- bounded protected latest-edition cache and least-privilege Keychain credential
- safe typed routes, Dynamic Type, VoiceOver identifiers, and Reduce Motion support
- deterministic demo fixtures plus Swift Package, app-unit, and UI tests

Search is reachable from Home; its entry reader remains intentionally basic
until the corpus search-and-view phase. Tasks remain outside this MVP. Remote
push is also not presented as live: the server
does not yet expose APNs device, outbox, inbox, or receipt contracts. See
`docs/ios/MVP.md` for that boundary and rollout plan.

## Open

```sh
open /Users/Shared/projects/straylight/apps/ios/Straylight.xcodeproj
```

Launch with `--demo` to bypass the owner-alpha credential screen and use the
deterministic briefing, news, archive, and topic fixtures.

## Verify

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

The checked-in Xcode project mirrors `project.yml`. The app itself has no
third-party runtime dependencies.

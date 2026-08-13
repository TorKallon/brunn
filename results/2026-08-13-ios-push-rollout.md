# iOS push rollout — 2026-08-13

## Outcome

Straylight's owner-alpha APNs transport is enabled for the signed development
app. Provider credentials are stored only on the Railway worker, and their
values are not recorded here. API and worker use the same enabled delivery
gate and the `com.rourkem.straylight` topic.

The connected `monolith` iPhone had already registered installation
`343d6ce5-cf18-4379-8a7f-63ff67a46f2d` successfully. Its signed Debug build
uses the development APNs environment.

## Deployment evidence

- Worker deployment `6b60b53e-fff7-44b6-8eb3-3f59071b4ec3` succeeded and
  logged `APNs notification delivery enabled` before the API gate was enabled.
- API deployment `9f532f28-a4dd-406e-8d98-36a59eec553e` succeeded with two
  running replicas; `/api/ready` reported `ready` with database, object store,
  and embeddings ready.
- The first post-variable-change canary was terminally suppressed because a
  Railway service restart reused the old deployment's environment snapshot.
  The API was then fully redeployed. This behavior is now explicit in the
  operations runbook.
- Fresh canary `notification:019ffb8158d77e339940586d88e69f18` created
  delivery `delivery:a492767e06024167815ca59a28ee7849`, which Apple accepted
  at `2026-08-13T14:23:06.311356Z` with no provider error.

The owner received that canary, but its first tap exposed an iOS client crash.
Straylight 0.2.0 build 4 aborted in the async notification-response delegate
while UIKit continued snapshot and state-restoration work on a cooperative
queue. The backend stayed healthy and later received the route after the app
relaunched.

Build 5 replaces the async delegate bridge with the explicit completion-handler
API, returns UIKit's response on the main queue before publishing SwiftUI
navigation, retains inactive-scene routes until activation, and resumes a
pending alert before the slower briefing refresh. The signed development build
was installed on `monolith` and verified with a fresh canary:

- Notification `notification:019ffbcc8bce7270882b5f37e66cc33a`
- Delivery `delivery:c635cd6c406145f184affc71576cb9ad`
- APNs accepted at `2026-08-13T15:45:14.468923Z`
- Delivery-attributed open recorded at `2026-08-13T15:48:36.430605Z`
- Owner acknowledgement recorded at `2026-08-13T15:48:38.373108Z`
- No new Straylight crash report appeared on the phone after the tap

## Verification

```text
python3 -m unittest tests.test_railway_contract -v
19 passed

swift test --package-path apps/ios --scratch-path /tmp/straylight-ios-spm-tap-fix
21 passed

xcodebuild ... -only-testing:StraylightTests test
passed

xcodebuild ... -sdk iphoneos ... build
passed; signed Debug build 0.2.0 (5), development APNs entitlement

git diff --check
passed
```

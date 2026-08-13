@testable import Straylight
import Foundation
import XCTest

final class StraylightTests: XCTestCase {
    @MainActor
    func testFirstRunShowsConnectionWithoutCallingTheAPI() async {
        let credentialStore = TestCredentialStore(token: nil)
        let calls = CallCounter()
        let model = AppModel(
            credentialStore: credentialStore,
            bootstrapIdentityLoader: { _ in
                await calls.increment()
                return Self.readOnlyIdentity
            }
        )

        await model.bootstrap()
        let callCount = await calls.value

        XCTAssertEqual(model.phase, .connectionRequired)
        XCTAssertEqual(callCount, 0)
    }

    @MainActor
    func testStoredSessionValidationStopsWaitingAtStartupBudget() async {
        let credentialStore = TestCredentialStore(token: "sl_stale")
        let model = AppModel(
            credentialStore: credentialStore,
            bootstrapValidationTimeout: .milliseconds(40),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in
                try await Task.sleep(for: .seconds(30))
                return Self.readOnlyIdentity
            }
        )

        await model.bootstrap()

        XCTAssertEqual(model.phase, .connectionRequired)
        XCTAssertTrue(credentialStore.wasDeleted)
        XCTAssertTrue(model.connectionMessage?.contains("taking too long") == true)
    }

    @MainActor
    func testDemoBootstrapsWithoutCredentialOrNetwork() {
        let model = AppModel()
        model.enterDemo()

        XCTAssertEqual(model.phase, .ready)
        XCTAssertTrue(model.isDemo)
        XCTAssertEqual(model.selectedTab, .dashboard)
        XCTAssertEqual(model.currentCredentialID, "credential:demo-iphone")
        XCTAssertEqual(model.dashboard?.storage.text.count, 4_926)
        XCTAssertEqual(model.dashboard?.access.count, 4)
        XCTAssertEqual(model.latestBriefing?.briefing?.schema, "briefing.v1")
        XCTAssertFalse(model.tasks.isEmpty)
        XCTAssertFalse(model.alerts.isEmpty)
    }

    @MainActor
    func testCachedBriefingDoesNotLoadDashboardBeforeCredentialValidation() async throws {
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("latest-briefing.json")
        defer {
            try? FileManager.default.removeItem(
                at: cacheURL.deletingLastPathComponent()
            )
        }
        let cache = BriefingCache(fileURL: cacheURL)
        try await cache.save(SampleData.briefing)
        let identityGate = IdentityGate()
        let dashboardCalls = CallCounter()
        let model = AppModel(
            api: Self.offlineAPI(),
            credentialStore: TestCredentialStore(token: "sl_cached"),
            briefingCache: cache,
            bootstrapValidationTimeout: .seconds(2),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in
                await identityGate.load()
            },
            dashboardLoader: { _, _ in
                await dashboardCalls.increment()
                return SampleData.dashboard
            }
        )

        let bootstrap = Task { await model.bootstrap() }
        for _ in 0 ..< 100 where !(model.phase == .ready && !model.connectionValidated) {
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertEqual(model.phase, .ready)
        XCTAssertFalse(model.connectionValidated)

        await model.refreshDashboard()
        let callCountBeforeValidation = await dashboardCalls.value
        XCTAssertEqual(callCountBeforeValidation, 0)

        await identityGate.resolve(with: Self.readOnlyIdentity)
        await bootstrap.value
    }

    @MainActor
    func testLateDashboardResponseCannotRepopulateAfterDisconnect() async throws {
        let dashboardGate = DashboardGate()
        let cacheURL = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
            .appendingPathComponent("latest-briefing.json")
        defer {
            try? FileManager.default.removeItem(
                at: cacheURL.deletingLastPathComponent()
            )
        }
        let model = AppModel(
            api: Self.offlineAPI(),
            credentialStore: TestCredentialStore(token: "sl_valid"),
            briefingCache: BriefingCache(fileURL: cacheURL),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in Self.readOnlyIdentity },
            dashboardLoader: { _, _ in try await dashboardGate.load() }
        )

        await model.bootstrap()
        for _ in 0 ..< 100 where !(await dashboardGate.hasStarted) {
            try await Task.sleep(for: .milliseconds(10))
        }
        let dashboardRequestStarted = await dashboardGate.hasStarted
        XCTAssertTrue(dashboardRequestStarted)

        await model.disconnect()
        await dashboardGate.resolve(with: SampleData.dashboard)
        try await Task.sleep(for: .milliseconds(30))

        XCTAssertNil(model.dashboard)
        XCTAssertNil(model.dashboardMessage)
        XCTAssertFalse(model.isRefreshingDashboard)
        XCTAssertEqual(model.phase, .connectionRequired)
    }

    func testDashboardFreshnessUsesLocalDayAndFallsBackOnInvalidTimestamp() throws {
        let timezone = try XCTUnwrap(TimeZone(identifier: "America/Los_Angeles"))
        let now = try XCTUnwrap(ISO8601DateFormatter().date(from: "2026-08-02T12:00:00Z"))

        XCTAssertFalse(AppModel.dashboardNeedsRefresh(
            Self.dashboard(generatedAt: "2026-08-02T11:59:00Z"),
            now: now,
            timezone: timezone
        ))
        XCTAssertTrue(AppModel.dashboardNeedsRefresh(
            Self.dashboard(generatedAt: "2026-08-02T11:50:00Z"),
            now: now,
            timezone: timezone
        ))
        XCTAssertTrue(AppModel.dashboardNeedsRefresh(
            Self.dashboard(generatedAt: "not-a-timestamp"),
            now: now,
            timezone: timezone
        ))

        let afterLocalMidnight = try XCTUnwrap(
            ISO8601DateFormatter().date(from: "2026-08-02T07:01:00Z")
        )
        XCTAssertTrue(AppModel.dashboardNeedsRefresh(
            Self.dashboard(generatedAt: "2026-08-02T06:59:00Z"),
            now: afterLocalMidnight,
            timezone: timezone
        ))
    }

    func testDashboardWeekdayKeepsCivilDateAcrossTimeZones() throws {
        let losAngeles = try XCTUnwrap(TimeZone(identifier: "America/Los_Angeles"))
        let tokyo = try XCTUnwrap(TimeZone(identifier: "Asia/Tokyo"))

        XCTAssertEqual(DashboardDate.shortDay("2026-08-02", timezone: losAngeles), "Sun")
        XCTAssertEqual(DashboardDate.shortDay("2026-08-02", timezone: tokyo), "Sun")
    }

    @MainActor
    func testPendingRouteSurvivesConnectionBoundary() async {
        let model = AppModel()
        let notificationRef = "notification:11111111111111111111111111111111"

        await model.handle(.notification(notificationRef: notificationRef, deliveryRef: nil))
        model.enterDemo()

        XCTAssertEqual(model.selectedTab, .alerts)
    }

    @MainActor
    func testPasswordLoginAcceptsOwnerSession() async {
        let owner = MeData(
            user: UserSummary(id: "user:one", displayName: "Owner"),
            credentialID: "credential:web-owner",
            capabilities: ["read", "credential:manage", "admin"],
            readOnly: false
        )
        let model = AppModel(
            api: Self.offlineAPI(),
            credentialStore: TestCredentialStore(token: "legacy-token"),
            loginLoader: { _, email, password in
                XCTAssertEqual(email, "rourkem@rourkem.com")
                XCTAssertEqual(password, "correct horse battery staple")
                return owner
            }
        )

        await model.connect(
            email: " rourkem@rourkem.com ",
            password: "correct horse battery staple"
        )

        XCTAssertEqual(model.phase, .ready)
        XCTAssertEqual(model.user, owner.user)
        XCTAssertEqual(model.currentCredentialID, owner.credentialID)
        XCTAssertTrue(model.connectionValidated)
        XCTAssertTrue(model.canManageNotifications)
    }

    func testNotificationMutationsUseAccountSessionAndCSRF() async throws {
        let host = "notification-\(UUID().uuidString.lowercased()).straylight.test"
        let baseURL = try XCTUnwrap(URL(string: "https://\(host)/api/v1"))
        let cookieStorage = HTTPCookieStorage.shared

        let sessionCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "straylight_session",
            .value: "session-secret",
            .secure: "TRUE",
        ]))
        let csrfCookie = try XCTUnwrap(HTTPCookie(properties: [
            .domain: host,
            .path: "/",
            .name: "straylight_csrf",
            .value: "csrf-secret",
            .secure: "TRUE",
        ]))
        cookieStorage.setCookie(sessionCookie)
        cookieStorage.setCookie(csrfCookie)
        defer {
            cookieStorage.deleteCookie(sessionCookie)
            cookieStorage.deleteCookie(csrfCookie)
            NotificationRequestURLProtocol.handler = nil
        }

        let recorder = NotificationRequestRecorder()
        NotificationRequestURLProtocol.handler = { request in
            recorder.append(request)
            if request.httpMethod == "PUT" {
                return StubbedHTTPResponse(json: #"""
                {"installation_ref":"installation:11111111111111111111111111111111","status":"active","updated_at":"2026-08-02T20:00:00Z"}
                """#)
            }
            return StubbedHTTPResponse(json: #"""
            {"notification_ref":"notification:11111111111111111111111111111111","kind":"opened","delivery_ref":"delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","recorded_at":"2026-08-02T20:01:00Z","replayed":false,"opened_at":"2026-08-02T20:01:00Z","acknowledged_at":null}
            """#)
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NotificationRequestURLProtocol.self]
        configuration.httpCookieStorage = cookieStorage
        configuration.httpShouldSetCookies = true
        let api = StraylightAPI(
            configuration: .init(baseURL: baseURL),
            session: URLSession(configuration: configuration),
            cookieStorage: cookieStorage
        )

        let hasSession = await api.hasAuthenticatedSession()
        XCTAssertTrue(hasSession)
        _ = try await api.upsertNotificationInstallation(
            installationID: UUID(uuidString: "11111111-1111-1111-1111-111111111111")!,
            request: NotificationInstallationRequest(
                environment: "development",
                appID: "com.rourkem.straylight",
                deviceToken: "00ff"
            )
        )
        _ = try await api.recordNotificationReceipt(
            notificationRef: "notification:11111111111111111111111111111111",
            kind: .opened,
            deliveryRef: "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )

        let requests = recorder.snapshot()
        XCTAssertEqual(requests.map(\.httpMethod), ["PUT", "POST"])
        XCTAssertTrue(requests.allSatisfy {
            $0.value(forHTTPHeaderField: "X-CSRF-Token") == "csrf-secret"
        })
        XCTAssertTrue(requests.allSatisfy {
            $0.value(forHTTPHeaderField: "Authorization") == nil
        })
        XCTAssertTrue(requests.allSatisfy {
            $0.value(forHTTPHeaderField: "Cookie")?.contains(
                "straylight_session=session-secret"
            ) == true
        })
        XCTAssertEqual(
            requests.first?.url?.path,
            "/api/v1/workspace/notification-installations/11111111-1111-1111-1111-111111111111"
        )
        XCTAssertEqual(
            requests.last?.url?.path,
            "/api/v1/workspace/notifications/notification:11111111111111111111111111111111/receipts"
        )
        let receiptBody = try XCTUnwrap(requests.last?.httpBody)
        let receiptObject = try XCTUnwrap(
            JSONSerialization.jsonObject(with: receiptBody) as? [String: Any]
        )
        XCTAssertEqual(receiptObject["kind"] as? String, "opened")
        XCTAssertEqual(
            receiptObject["delivery_ref"] as? String,
            "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
    }

    private static let readOnlyIdentity = MeData(
        user: UserSummary(id: "user:one", displayName: "Owner"),
        capabilities: ["open", "query", "read", "compute", "verify", "status"],
        readOnly: true
    )

    private static let notificationIdentity = MeData(
        user: UserSummary(id: "user:one", displayName: "Owner"),
        credentialID: "credential:web-owner",
        capabilities: ["query", "read", "status", "notification:manage"],
        readOnly: false
    )

    private static func testNotification(openedAt: String? = nil) -> StraylightNotification {
        StraylightNotification(
            notificationRef: "notification:11111111111111111111111111111111",
            kind: .briefingReady,
            importance: .important,
            title: "Morning briefing ready",
            body: "Open the durable detail before continuing.",
            source: StraylightNotificationSource(
                type: "entry",
                reference: "entry:morning",
                versionRef: "version:morning-v3"
            ),
            target: StraylightNotificationTarget(
                type: .briefing,
                date: "2026-08-02",
                edition: "morning",
                itemID: "native-ios"
            ),
            occurredAt: "2026-08-02T06:30:00Z",
            openedAt: openedAt,
            deliveries: [
                StraylightNotificationDelivery(
                    deliveryRef: "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    state: .acceptedByAPNs,
                    acceptedAt: "2026-08-02T06:30:02Z"
                ),
            ]
        )
    }

    private static func offlineAPI() -> StraylightAPI {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.waitsForConnectivity = false
        configuration.timeoutIntervalForRequest = 0.1
        configuration.timeoutIntervalForResource = 0.1
        return StraylightAPI(
            configuration: .init(
                baseURL: URL(string: "http://127.0.0.1:9/api/v1")!
            ),
            session: URLSession(configuration: configuration)
        )
    }

    private static func dashboard(generatedAt: String) -> WorkspaceDashboardData {
        let dashboard = SampleData.dashboard
        return WorkspaceDashboardData(
            generatedAt: generatedAt,
            timezone: dashboard.timezone,
            workspaceGeneration: dashboard.workspaceGeneration,
            activityTrackingStartedAt: dashboard.activityTrackingStartedAt,
            tracking: dashboard.tracking,
            storage: dashboard.storage,
            today: dashboard.today,
            activity: dashboard.activity,
            access: dashboard.access,
            coverage: dashboard.coverage
        )
    }

    func testNotificationParserRejectsArbitraryPayload() {
        XCTAssertNil(NotificationRouteParser.route(from: ["url": "https://example.com"]))
    }

    func testNotificationParserAcceptsTypedRoute() {
        let notificationID = "11111111111111111111111111111111"
        let deliveryID = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        XCTAssertEqual(
            NotificationRouteParser.route(
                from: [
                    "schema": "straylight-push@v1",
                    "notification_ref": "notification:\(notificationID)",
                    "delivery_ref": "delivery:\(deliveryID)",
                    "straylight_route": "straylight://notification/\(notificationID)?delivery=\(deliveryID)",
                ]
            ),
            .notification(
                notificationRef: "notification:\(notificationID)",
                deliveryRef: "delivery:\(deliveryID)"
            )
        )
    }

    func testNotificationParserRejectsMismatchedOrUnversionedPayloads() {
        let notificationID = "11111111111111111111111111111111"
        let deliveryID = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        let route = "straylight://notification/\(notificationID)?delivery=\(deliveryID)"

        XCTAssertNil(NotificationRouteParser.route(from: [
            "notification_ref": "notification:\(notificationID)",
            "delivery_ref": "delivery:\(deliveryID)",
            "straylight_route": route,
        ]))
        XCTAssertNil(NotificationRouteParser.route(from: [
            "schema": "straylight-push@v1",
            "notification_ref": "notification:22222222222222222222222222222222",
            "delivery_ref": "delivery:\(deliveryID)",
            "straylight_route": route,
        ]))
    }

    func testPushRouteBufferRetainsOneColdLaunchRoute() {
        _ = PushRouteBuffer.shared.take()
        let route = AppRoute.notification(
            notificationRef: "notification:11111111111111111111111111111111",
            deliveryRef: "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )

        PushRouteBuffer.shared.store(route)

        XCTAssertEqual(PushRouteBuffer.shared.take(), route)
        XCTAssertNil(PushRouteBuffer.shared.take())
    }

    func testNotificationResponseCompletesOnMainBeforePublishingBufferedRoute() async {
        _ = PushRouteBuffer.shared.take()
        let route = AppRoute.notification(
            notificationRef: "notification:11111111111111111111111111111111",
            deliveryRef: "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
        let completed = expectation(description: "notification response completed")
        let published = expectation(description: "push route published")
        let events = NotificationHandoffRecorder()
        let observer = NotificationCenter.default.addObserver(
            forName: .straylightPushRoute,
            object: nil,
            queue: .main
        ) { _ in
            events.append("published")
            published.fulfill()
        }
        defer { NotificationCenter.default.removeObserver(observer) }

        NotificationDelegateHandoff.finishResponse(route: route) {
            XCTAssertTrue(Thread.isMainThread)
            XCTAssertEqual(PushRouteBuffer.shared.take(), route)
            events.append("completed")
            completed.fulfill()
        }

        await fulfillment(of: [completed, published], timeout: 1)
        XCTAssertEqual(events.snapshot(), ["completed", "published"])
    }

    @MainActor
    func testAppDelegateExportsExplicitNotificationCompletionSelectors() {
        let delegate = AppDelegate()

        XCTAssertTrue(delegate.responds(to: NSSelectorFromString(
            "userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:"
        )))
        XCTAssertTrue(delegate.responds(to: NSSelectorFromString(
            "userNotificationCenter:willPresentNotification:withCompletionHandler:"
        )))
    }

    func testPushTokenBufferRetainsOnlyInMemoryUntilConsumed() {
        _ = PushTokenBuffer.shared.take()
        let token = Data([0x00, 0x7f, 0xff])

        PushTokenBuffer.shared.store(token)

        XCTAssertEqual(PushTokenBuffer.shared.take(), token)
        XCTAssertNil(PushTokenBuffer.shared.take())
    }

    @MainActor
    func testColdAndWarmPushTapFetchDetailBeforePresentationAndAttributeDeliveryOpen() async {
        let trace = NotificationTrace()
        let notification = Self.testNotification(openedAt: "2026-08-02T06:40:00Z")
        let deliveryRef = "delivery:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        let model = AppModel(
            api: Self.offlineAPI(),
            credentialStore: TestCredentialStore(token: nil),
            storedSessionChecker: { _ in true },
            bootstrapIdentityLoader: { _ in Self.notificationIdentity },
            dashboardLoader: { _, _ in SampleData.dashboard },
            notificationListLoader: { _, _ in
                NotificationListResponse(items: [notification], unreadCount: 0)
            },
            notificationDetailLoader: { _, reference in
                await trace.append("detail:\(reference)")
                return notification
            },
            notificationReceiptWriter: { _, reference, kind, delivery in
                await trace.append("receipt:\(delivery ?? "none")")
                return NotificationReceiptResponse(
                    notificationRef: reference,
                    kind: kind,
                    deliveryRef: delivery,
                    recordedAt: "2026-08-02T06:41:00Z",
                    replayed: false,
                    openedAt: "2026-08-02T06:40:00Z",
                    acknowledgedAt: nil
                )
            }
        )

        await model.handle(.notification(
            notificationRef: notification.notificationRef,
            deliveryRef: deliveryRef
        ))
        XCTAssertNil(model.presentedNotification)

        await model.bootstrap()

        XCTAssertEqual(model.selectedTab, .alerts)
        XCTAssertEqual(model.presentedNotification?.notificationRef, notification.notificationRef)
        XCTAssertNil(model.focusedBriefingItemID, "A push must not bypass durable alert detail.")
        let coldTrace = await trace.values
        XCTAssertEqual(coldTrace, [
            "detail:\(notification.notificationRef)",
            "receipt:\(deliveryRef)",
        ])

        model.presentedNotification = nil
        await model.handle(.notification(
            notificationRef: notification.notificationRef,
            deliveryRef: nil
        ))
        XCTAssertEqual(model.presentedNotification?.notificationRef, notification.notificationRef)
        let warmTrace = await trace.values
        XCTAssertEqual(warmTrace.last, "detail:\(notification.notificationRef)")
    }

    @MainActor
    func testNotificationTargetIsASecondaryActionToExactBriefingItem() async throws {
        let model = AppModel()
        model.enterDemo()
        let notification = try XCTUnwrap(SampleData.notifications.first)

        await model.openNotification(reference: notification.notificationRef)

        XCTAssertEqual(model.selectedTab, .alerts)
        XCTAssertEqual(model.presentedNotification?.notificationRef, notification.notificationRef)
        XCTAssertNil(model.focusedBriefingItemID)

        await model.openNotificationTarget(notification)

        XCTAssertEqual(model.selectedTab, .today)
        XCTAssertEqual(model.focusedBriefingItemID, "ios-direction")
        XCTAssertNil(model.presentedNotification)
    }

    @MainActor
    func testEntryTargetUsesPinnedVersionReferenceForExactRead() async throws {
        let model = AppModel()
        model.enterDemo()
        let notification = try XCTUnwrap(
            SampleData.notifications.first(where: { $0.target.type == .entry })
        )

        let item = try await model.readNotificationEntry(notification)

        XCTAssertEqual(item.reference, notification.source?.versionRef)
    }

    func testSafeMarkdownAllowsWebLinksAndRemovesOtherSchemes() {
        let value = SafeMarkdown.attributedString(
            "[web](https://straylight.rourkem.com) [unsafe](javascript:alert(1))"
        )
        let links = value.runs.compactMap(\.link)

        XCTAssertEqual(links.map(\.scheme), ["https"])
    }

    func testEntryMarkdownKeepsWebLinksAndRoutesRelativeAndWikiLinksInternally() throws {
        let value = SafeMarkdown.entryAttributedString(
            "[web](https://straylight.rourkem.com) [relative](../Other.md) [[Topics/Gaming/Gaming|Gaming]]"
        )
        let links = value.runs.compactMap(\.link)

        XCTAssertEqual(links.map(\.scheme), ["https", "straylight-entry", "straylight-entry"])
        let relative = try XCTUnwrap(EntryNavigationURL.link(from: links[1]))
        XCTAssertEqual(relative.target, "../Other.md")
        XCTAssertFalse(relative.isWikiLink)
        let wiki = try XCTUnwrap(EntryNavigationURL.link(from: links[2]))
        XCTAssertEqual(wiki.target, "Topics/Gaming/Gaming")
        XCTAssertEqual(wiki.label, "Gaming")
        XCTAssertTrue(wiki.isWikiLink)
    }

    func testEntryMarkdownDoesNotRewriteWikiSyntaxInsideCode() {
        let markdown = """
        `[[Inline literal]]`

        ```swift
        let example = "[[Fenced literal]]"
        ```

        ~~~text
        [[Tilde fenced literal]]
        ~~~

        > ```text
        > [[Blockquoted fenced literal]]
        > ```

        `[[Multiline
        code literal]]`

            [[Indented literal]]

        [[Real entry]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("`[[Inline literal]]`"))
        XCTAssertTrue(rewritten.contains("[[Fenced literal]]"))
        XCTAssertTrue(rewritten.contains("[[Tilde fenced literal]]"))
        XCTAssertTrue(rewritten.contains("[[Blockquoted fenced literal]]"))
        XCTAssertTrue(rewritten.contains("`[[Multiline\ncode literal]]`"))
        XCTAssertTrue(rewritten.contains("    [[Indented literal]]"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "straylight-entry://open").count - 1,
            1
        )
        XCTAssertEqual(
            SafeMarkdown.entryAttributedString(markdown).runs.compactMap(\.link).count,
            1
        )
    }

    func testEntryMarkdownLeavesUnmatchedBackticksLiteralWithoutHidingLaterWikiLinks() {
        let markdown = """
        An unmatched ` delimiter stays literal.
        An unmatched `` delimiter also stays literal.
        [[Still linked]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("unmatched ` delimiter"))
        XCTAssertTrue(rewritten.contains("unmatched `` delimiter"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "straylight-entry://open").count - 1,
            1
        )
        let rendered = SafeMarkdown.entryAttributedString(markdown)
        XCTAssertTrue(String(rendered.characters).contains("unmatched ` delimiter"))
        XCTAssertTrue(String(rendered.characters).contains("unmatched `` delimiter"))
        XCTAssertEqual(rendered.runs.compactMap(\.link).count, 1)
    }

    func testEntryMarkdownDoesNotRewriteWikiSyntaxInsideListNestedFences() {
        let markdown = """
        - ```markdown
          [[Bullet literal]]
          ```

        1. ~~~markdown
           [[Ordered literal]]
           ~~~

        + ```markdown
          [[Plus bullet literal]]
          ```

        2) ~~~markdown
           [[Parenthesized ordered literal]]
           ~~~

        -\t```markdown
        \t[[Tabbed bullet literal]]
        \t```

          - ```markdown
            [[Nested bullet literal]]
            ```

        > - ```markdown
        >   [[Blockquote bullet literal]]
        >   ```

        - > ~~~markdown
          > [[Bullet blockquote literal]]
          > ~~~

        [[Real entry]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("[[Bullet literal]]"))
        XCTAssertTrue(rewritten.contains("[[Ordered literal]]"))
        XCTAssertTrue(rewritten.contains("[[Plus bullet literal]]"))
        XCTAssertTrue(rewritten.contains("[[Parenthesized ordered literal]]"))
        XCTAssertTrue(rewritten.contains("[[Tabbed bullet literal]]"))
        XCTAssertTrue(rewritten.contains("[[Nested bullet literal]]"))
        XCTAssertTrue(rewritten.contains("[[Blockquote bullet literal]]"))
        XCTAssertTrue(rewritten.contains("[[Bullet blockquote literal]]"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "straylight-entry://open").count - 1,
            1
        )
    }

    func testEntryMarkdownRestartsFenceWhenBlockquoteContainerEnds() {
        let markdown = """
        > ```markdown
        > [[Blockquote literal]]
        ```markdown
        [[Top-level literal]]
        ```
        [[Real entry]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("[[Blockquote literal]]"))
        XCTAssertTrue(rewritten.contains("[[Top-level literal]]"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "straylight-entry://open").count - 1,
            1
        )
    }

    func testEntryMarkdownRestartsFenceWhenListContainerEnds() {
        let markdown = """
        - ```markdown
          [[List literal]]
        ```markdown
        [[Top-level literal]]
        ```
        [[Real entry]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("[[List literal]]"))
        XCTAssertTrue(rewritten.contains("[[Top-level literal]]"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "straylight-entry://open").count - 1,
            1
        )
    }

    func testEntryMarkdownKeepsNestedContainerFenceMarkersInsideTopLevelFence() {
        let markdown = """
        ```markdown
        [[Top-level literal]]
        > ```markdown
        > [[Blockquote-looking literal]]
        > ```
        - ```markdown
        - [[List-looking literal]]
        - ```
        [[Still top-level literal]]
        ```
        [[Real entry]]
        """

        let rewritten = EntryMarkdown.rewritingWikiLinks(in: markdown)

        XCTAssertTrue(rewritten.contains("[[Blockquote-looking literal]]"))
        XCTAssertTrue(rewritten.contains("[[List-looking literal]]"))
        XCTAssertTrue(rewritten.contains("[[Still top-level literal]]"))
        XCTAssertEqual(
            rewritten.components(separatedBy: "straylight-entry://open").count - 1,
            1
        )
    }

    @MainActor
    func testDemoSearchEntryReadsPinnedMarkdownAndFollowsInternalLink() async throws {
        let model = AppModel()
        model.enterDemo()
        let first = try XCTUnwrap(SampleData.searchResults.first)

        let item = try await model.read(first)
        XCTAssertEqual(item.reference, first.reference)
        XCTAssertEqual(item.version, first.version)
        XCTAssertTrue(item.text?.contains("[[") == true)

        let attributed = SafeMarkdown.entryAttributedString(try XCTUnwrap(item.text))
        let internalURL = try XCTUnwrap(
            attributed.runs.compactMap(\.link).first(where: {
                $0.scheme == "straylight-entry"
            })
        )
        let link = try XCTUnwrap(EntryNavigationURL.link(from: internalURL))
        let linked = try await model.read(WorkspaceEntryRequest(link: link, sourcePath: item.path))

        XCTAssertNotEqual(linked.reference, item.reference)
        XCTAssertEqual(linked.title, SampleData.searchResults.dropFirst().first?.title)
    }

    @MainActor
    func testShortSearchClearsPreviouslyDisplayedResults() async {
        let model = AppModel()
        model.enterDemo()

        await model.performSearch("straylight")
        XCTAssertFalse(model.searchResults.isEmpty)

        await model.performSearch("x")
        XCTAssertTrue(model.searchResults.isEmpty)
        XCTAssertEqual(model.searchMessage, "Enter at least two characters.")
    }

    @MainActor
    func testDemoFixtureCoversTheCompleteReaderAndNewsDeliveryStates() throws {
        let model = AppModel()
        model.enterDemo()

        let briefing = try XCTUnwrap(model.latestBriefing)
        let payload = try XCTUnwrap(briefing.briefing)
        let sections = try XCTUnwrap(payload.sections)
        let items = sections.flatMap(\.items)

        XCTAssertEqual(payload.summaryMD?.count, 7)
        XCTAssertEqual(sections.map(\.topic), ["straylight", "platform", "reading-experience"])
        XCTAssertEqual(
            items.map(\.id),
            ["ios-direction", "existing-contracts", "delivery-correction", "full-width-reader"]
        )
        XCTAssertTrue(items.allSatisfy { !($0.story?.urls ?? []).isEmpty })
        XCTAssertEqual(model.newsItems.count, items.count)
        XCTAssertTrue(model.newsItems.contains { $0.kind == .new })
        XCTAssertTrue(model.newsItems.contains { $0.kind == .update })
        XCTAssertTrue(model.newsItems.contains { $0.kind == .correction })
        XCTAssertTrue(model.newsItems.allSatisfy(\.isPriority))
        XCTAssertEqual(model.briefingHistory.map(\.date), ["2026-08-02", "2026-08-01", "2026-08-01"])
        XCTAssertEqual(model.briefingHistory.map(\.edition), ["morning", "evening", "morning"])
        XCTAssertEqual(model.topicsSnapshot?.topics.count, 2)
        XCTAssertEqual(model.topicsSnapshot?.pendingRequests.count, 1)
        XCTAssertTrue(model.isNewsItemRead("existing-contracts"))
        XCTAssertFalse(model.isNewsItemRead("ios-direction"))
    }

    @MainActor
    func testMarkingNewsReadChangesOnlySessionPresentationState() {
        let model = AppModel()
        model.enterDemo()
        let before = model.newsItems

        model.markNewsItemRead("ios-direction")

        XCTAssertTrue(model.isNewsItemRead("ios-direction"))
        XCTAssertEqual(model.newsItems, before)
        XCTAssertEqual(model.latestBriefing, SampleData.briefing)
    }
}

@MainActor
private final class TestCredentialStore: CredentialStoring {
    private var token: String?
    private(set) var wasDeleted = false

    init(token: String?) {
        self.token = token
    }

    func load() throws -> String? {
        token
    }

    func save(_ token: String) throws {
        self.token = token
    }

    func delete() throws {
        wasDeleted = true
        token = nil
    }
}

private actor CallCounter {
    private(set) var value = 0

    func increment() {
        value += 1
    }
}

private actor NotificationTrace {
    private(set) var values: [String] = []

    func append(_ value: String) {
        values.append(value)
    }
}

private final class NotificationHandoffRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var events: [String] = []

    func append(_ event: String) {
        lock.lock()
        events.append(event)
        lock.unlock()
    }

    func snapshot() -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return events
    }
}

private struct StubbedHTTPResponse: Sendable {
    let statusCode: Int
    let json: String

    init(statusCode: Int = 200, json: String) {
        self.statusCode = statusCode
        self.json = json
    }
}

private final class NotificationRequestRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var requests: [URLRequest] = []

    func append(_ request: URLRequest) {
        lock.lock()
        requests.append(request)
        lock.unlock()
    }

    func snapshot() -> [URLRequest] {
        lock.lock()
        defer { lock.unlock() }
        return requests
    }
}

private final class NotificationRequestURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> StubbedHTTPResponse)?

    override class func canInit(with _: URLRequest) -> Bool { true }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let handler = Self.handler, let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        var capturedRequest = request
        if capturedRequest.httpBody == nil,
           let bodyStream = capturedRequest.httpBodyStream
        {
            bodyStream.open()
            defer { bodyStream.close() }
            var body = Data()
            let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: 4096)
            defer { buffer.deallocate() }
            while bodyStream.hasBytesAvailable {
                let count = bodyStream.read(buffer, maxLength: 4096)
                guard count > 0 else { break }
                body.append(buffer, count: count)
            }
            capturedRequest.httpBody = body
        }
        let stub = handler(capturedRequest)
        guard let response = HTTPURLResponse(
            url: url,
            statusCode: stub.statusCode,
            httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"]
        ) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(stub.json.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

private actor IdentityGate {
    private var continuation: CheckedContinuation<MeData, Never>?

    func load() async -> MeData {
        await withCheckedContinuation { continuation in
            self.continuation = continuation
        }
    }

    func resolve(with identity: MeData) {
        continuation?.resume(returning: identity)
        continuation = nil
    }
}

private actor DashboardGate {
    private var continuation: CheckedContinuation<WorkspaceDashboardData, any Error>?
    private(set) var hasStarted = false

    func load() async throws -> WorkspaceDashboardData {
        hasStarted = true
        return try await withCheckedThrowingContinuation { continuation in
            self.continuation = continuation
        }
    }

    func resolve(with dashboard: WorkspaceDashboardData) {
        continuation?.resume(returning: dashboard)
        continuation = nil
    }
}

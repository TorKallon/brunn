@testable import Straylight
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
    func testStoredCredentialValidationStopsWaitingAtStartupBudget() async {
        let credentialStore = TestCredentialStore(token: "sl_stale")
        let model = AppModel(
            credentialStore: credentialStore,
            bootstrapValidationTimeout: .milliseconds(40),
            bootstrapIdentityLoader: { _ in
                try await Task.sleep(for: .seconds(30))
                return Self.readOnlyIdentity
            }
        )

        await model.bootstrap()

        XCTAssertEqual(model.phase, .connectionRequired)
        XCTAssertFalse(credentialStore.wasDeleted)
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

        await model.handle(.alert(id: "delivery:one"))
        model.enterDemo()

        XCTAssertEqual(model.selectedTab, .news)
    }

    func testOnlyLeastPrivilegeReadOnlyCredentialIsAccepted() {
        let readOnly = Self.readOnlyIdentity
        let owner = MeData(
            user: UserSummary(id: "user:one", displayName: "Owner"),
            capabilities: ["read", "credential:manage", "admin"]
        )
        let readWrite = MeData(
            user: UserSummary(id: "user:one", displayName: "Owner"),
            capabilities: ["query", "read", "status", "save"],
            readOnly: false
        )

        XCTAssertTrue(AppModel.isAllowedDeviceCredential(readOnly))
        XCTAssertFalse(AppModel.isAllowedDeviceCredential(owner))
        XCTAssertFalse(AppModel.isAllowedDeviceCredential(readWrite))
    }

    private static let readOnlyIdentity = MeData(
        user: UserSummary(id: "user:one", displayName: "Owner"),
        capabilities: ["open", "query", "read", "compute", "verify", "status"],
        readOnly: true
    )

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
        XCTAssertEqual(
            NotificationRouteParser.route(
                from: ["straylight_route": "straylight://alert/019fc3f1-opaque"]
            ),
            .alert(id: "019fc3f1-opaque")
        )
    }

    func testPushRouteBufferRetainsOneColdLaunchRoute() {
        _ = PushRouteBuffer.shared.take()
        let route = AppRoute.alert(id: "delivery:one")

        PushRouteBuffer.shared.store(route)

        XCTAssertEqual(PushRouteBuffer.shared.take(), route)
        XCTAssertNil(PushRouteBuffer.shared.take())
    }

    func testSafeMarkdownAllowsWebLinksAndRemovesOtherSchemes() {
        let value = SafeMarkdown.attributedString(
            "[web](https://straylight.rourkem.com) [unsafe](javascript:alert(1))"
        )
        let links = value.runs.compactMap(\.link)

        XCTAssertEqual(links.map(\.scheme), ["https"])
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

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
        XCTAssertEqual(model.latestBriefing?.briefing?.schema, "briefing.v1")
        XCTAssertFalse(model.tasks.isEmpty)
        XCTAssertFalse(model.alerts.isEmpty)
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

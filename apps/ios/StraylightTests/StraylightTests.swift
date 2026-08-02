@testable import Straylight
import XCTest

final class StraylightTests: XCTestCase {
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
        let readOnly = MeData(
            user: UserSummary(id: "user:one", displayName: "Owner"),
            capabilities: ["open", "query", "read", "compute", "verify", "status"],
            readOnly: true
        )
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

import XCTest

@MainActor
final class DocumentReaderUITests: XCTestCase {
    func testColdPinnedLinkOpenLatestAndExplicitSharing() throws {
        let app = demo()
        app.open(try XCTUnwrap(URL(string: "brunn://document/demo-handoff?version=1")))
        XCTAssertTrue(app.staticTexts["document-title"].waitForExistence(timeout: 5))
        XCTAssertEqual(app.staticTexts["document-version-notice"].label, "Historical version 1 of 2")
        screenshot("document-historical-dark", app)
        try app.performAccessibilityAudit(for: [.contrast, .textClipped, .trait])
        app.buttons["document-open-latest"].tap()
        XCTAssertTrue(app.buttons["document-share-menu"].waitForExistence(timeout: 3))
        XCTAssertFalse(app.staticTexts["document-version-notice"].exists)
        app.buttons["document-share-menu"].tap()
        XCTAssertTrue(app.buttons["Copy app link"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["Share app link"].exists)
        XCTAssertTrue(app.buttons["Share pinned version 2"].exists)
        app.buttons["Copy app link"].tap()
        app.buttons["document-dismiss"].tap()
        XCTAssertTrue(app.tabBars.buttons["Home"].waitForExistence(timeout: 3))
    }

    func testLongHandoffLightLargeTypeKeepsTablesCodeAndFinalDetail() throws {
        let app = demo(extra: ["-brunn.appearance.v1", "light", "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"])
        app.open(try XCTUnwrap(URL(string: "brunn://document/demo-handoff")))
        XCTAssertTrue(app.staticTexts["document-title"].waitForExistence(timeout: 5))
        screenshot("document-light-accessibility-title", app)
        try app.performAccessibilityAudit(for: [.contrast, .textClipped, .trait])
        let tableDetail = app.staticTexts["The entire handoff stays readable without clipping, even with larger text."]
        scroll(tableDetail, app)
        XCTAssertTrue(tableDetail.isHittable)
        XCTAssertLessThanOrEqual(tableDetail.frame.maxX, app.frame.maxX)
        screenshot("document-light-accessibility-table", app)
        let final = app.staticTexts["Final detail: the complete handoff is preserved."]
        scroll(final, app)
        XCTAssertTrue(final.isHittable)
        screenshot("document-light-accessibility-final", app)
        let privateSource = app.staticTexts["Private source record"]
        scroll(privateSource, app)
        XCTAssertTrue(privateSource.exists)
        XCTAssertFalse(app.links["Private source record"].exists)
        XCTAssertTrue(app.descendants(matching: .any).matching(identifier: "document-external-source-0").firstMatch.exists)
    }

    func testWarmLinksReplaceReaderAndDatedBriefingStaysABriefing() throws {
        let app = demo()
        app.launch()
        app.open(try XCTUnwrap(URL(string: "brunn://document/demo-handoff")))
        XCTAssertTrue(app.staticTexts["document-title"].waitForExistence(timeout: 5))
        app.open(try XCTUnwrap(URL(string: "brunn://document/demo-handoff?version=1")))
        XCTAssertTrue(app.staticTexts["document-version-notice"].waitForExistence(timeout: 3))
        app.open(try XCTUnwrap(URL(string: "brunn://briefing/2026-09-09/morning")))
        XCTAssertTrue(app.navigationBars["Today"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["document-title"].exists)
        screenshot("document-to-existing-briefing-route", app)
    }

    func testUnavailablePinnedRevisionNeverSubstitutesLatest() throws {
        let app = demo()
        app.open(try XCTUnwrap(URL(string: "brunn://document/demo-handoff?version=99")))
        XCTAssertTrue(app.staticTexts["Document unavailable"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Version 99 is not available to your account. No other revision was opened."].exists)
        XCTAssertFalse(app.buttons["document-open-latest"].exists)
        XCTAssertFalse(app.staticTexts["document-title"].exists)
        app.buttons["document-retry"].tap()
        XCTAssertTrue(app.staticTexts["Document unavailable"].waitForExistence(timeout: 3))
        screenshot("document-unavailable-revision", app)
    }

    func testRealTargetSlugsPreservedOnSignedOutColdAndWarmLaunch() throws {
        let app = XCUIApplication()
        app.launchArguments = ["--ui-test-connection-required", "-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.open(try XCTUnwrap(URL(string: "brunn://document/chief-of-staff-brief")))
        let notice = app.staticTexts["document-sign-in-destination"]
        XCTAssertTrue(notice.waitForExistence(timeout: 5))
        XCTAssertTrue(notice.label.contains("chief-of-staff-brief"))
        app.open(try XCTUnwrap(URL(string: "brunn://document/brunn-ios-document-deep-links-handoff?version=1")))
        XCTAssertTrue(notice.waitForExistence(timeout: 3))
        XCTAssertTrue(notice.label.contains("brunn-ios-document-deep-links-handoff · version 1"))
        screenshot("document-auth-required-real-slugs", app)
        // This verifies OS URL dispatch and the protected sign-in destination, not a
        // Messages tap or authenticated rendering of the owner's real documents.
    }

    private func demo(extra: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["--demo", "-AppleLanguages", "(en)", "-AppleLocale", "en_US", "-brunn.appearance.v1", "dark"] + extra
        return app
    }

    private func scroll(_ element: XCUIElement, _ app: XCUIApplication) {
        for _ in 0 ..< 40 {
            if element.isHittable { return }
            app.swipeUp()
        }
    }

    private func screenshot(_ name: String, _ app: XCUIApplication) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
